#[macro_use] extern crate log;
extern crate env_logger;
extern crate imap;
extern crate openssl;
#[macro_use(quick_error)] extern crate quick_error;
extern crate clap;

use openssl::ssl::{SslContext, SslMethod, SslStream};
use imap::client::Client;

use std::net::{TcpStream};
use std::sync::mpsc::*;
use std::fs::File;
use std::sync::Mutex;
use std::rc::Rc;

fn main() {
    env_logger::init().unwrap();
    let (host, username, password, email_folder, output_folder, starting_number) = input_parameters();

    let mut imap_socket = imap_client(&host, &username, &password);
    let inbox = imap_socket.select(&email_folder).unwrap();
    debug!("{}", &inbox);

    let saver = EmailSaver::new(&output_folder);

    for email in imap_socket.into_email_iter(&email_folder)
        .skip(starting_number)
        .filter(from_or_to_ukeep)
        .flat_map(into_email) {
            saver.save(email);
    }
}

fn from_or_to_ukeep(email_provider: &EmailProvider) -> bool {
    match (email_provider.to(), email_provider.from()) {
        (Ok(to), Ok(from)) => {
            to.contains("ukeep") || from.contains("ukeep")
        },
        _ => false
    }
}

fn into_email(email_provider: EmailProvider) -> Option<Email> {
    match email_provider.email() {
        Ok(email) => Some(email),
        Err(err) => {
            error!("{}", err);
            None
        }
    }
}

fn imap_client(host: &str, username: &str, password: &str) -> ImapClient {

    let mut imap_socket = Client::secure_connect(
        (host, 993), 
        SslContext::new(SslMethod::Sslv23).unwrap()
    ).unwrap();

    imap_socket.login(username, password).unwrap();

    let capabilities = imap_socket.capability().unwrap();
    for capability in capabilities.iter() {
	    trace!("capability {}", capability); 
    }

    imap_socket
}

trait EmailIter {
    fn into_email_iter(self, folder_name: &str) -> EmailFolderIterator;
}

impl EmailIter for ImapClient {
    fn into_email_iter(self, folder_name: &str) -> EmailFolderIterator {
        EmailFolderIterator::new(self, folder_name)
    }
}

struct EmailFolderIterator {
    imap: Rc<Mutex<ImapClient>>,
    total_messages: u32,
    current_message: u32
}

impl EmailFolderIterator {
    fn new(mut imap: ImapClient, email_folder: &str) -> EmailFolderIterator {
        let total_messages = imap.select(email_folder).unwrap().exists;
        EmailFolderIterator {
            imap: Rc::new(Mutex::new(imap)),
            total_messages: total_messages,
            current_message: 0
        }
    }
}

impl Iterator for EmailFolderIterator {
    type Item = EmailProvider;
    fn next(&mut self) -> Option<Self::Item> {
        if self.current_message >= self.total_messages {
            return None;
        }
        self.current_message += 1;
        Some(EmailProvider::new(self.imap.clone(), self.current_message))
    }
}

struct EmailProvider {
    imap: Rc<Mutex<ImapClient>>,
    number: u32
}

impl EmailProvider {
    fn new(imap: Rc<Mutex<ImapClient>>, email_number: u32) -> EmailProvider {
        EmailProvider {
            imap: imap,
            number: email_number
        }
    }

    fn from(&self) -> Result<FileContent, EmailError> {
        trace!("fetching 'from': {}", self.number);
        header_from(self.number, &mut *self.imap.lock().unwrap()).ok_or(EmailError::FailedFetch)
    }

    fn to(&self) -> Result<FileContent, EmailError> {
        trace!("fetching 'to': {}", self.number);
        header_to(self.number, &mut *self.imap.lock().unwrap()).ok_or(EmailError::FailedFetch)
    }

    fn subject(&self) -> Result<FileContent, EmailError> {
        trace!("fetching 'subject': {}", self.number);
        header_subject(self.number, &mut *self.imap.lock().unwrap()).ok_or(EmailError::FailedFetch)
    }

    fn body(&self) -> Result<FileContent, EmailError> {
        trace!("fetching 'body': {}", self.number);
        body(self.number, &mut *self.imap.lock().unwrap()).ok_or(EmailError::FailedFetch)
    }

    fn email(&self) -> Result<Email, EmailError> {
        Ok(Email {
            number: self.number,
            from: self.from()?,
            to: self.to()?,
            subject: self.subject()?,
            body: self.body()?
        })
    }
}

struct Email {
    number: u32,
    from: FileContent,
    to: FileContent,
    subject: FileContent,
    body: FileContent
}

struct EmailSaver {
    send: Sender<Email>
}

impl EmailSaver {
    fn new(output_folder_name: &str) -> Self {
        let (send, recv) = channel();
        let path = output_folder_name.to_owned();
        ::std::thread::spawn(move|| {
            ::std::fs::create_dir_all(&path).unwrap();
            while let Ok(email) = recv.recv() {
                match Self::save_to_file(email, &path) {
                    Err(err) => error!("error saving to file: {}", err),
                    _ => {}
                }
            }
        });
        EmailSaver {
            send: send
        }
    }

    fn save(&self, email: Email) {
        debug!("Saving email {}", email.number);
        self.send.send(email).unwrap();
    }

    fn save_to_file(email: Email, path: &str) -> Result<(), EmailError> {
        let from = try!( File::create(format!("{}/{}_from.txt", path, email.number)) );
        let to = try!( File::create(format!("{}/{}_to.txt", path, email.number)) );
        let subject = try!( File::create(format!("{}/{}_subject.txt", path, email.number)) );
        let body = try!( File::create(format!("{}/{}_body.txt", path, email.number)) );
        try!( write_all(from, email.from) );
        try!( write_all(to, email.to) );
        try!( write_all(subject, email.subject) );
        try!( write_all(body, email.body) );
        Ok(())
    }
}

fn header_from(message_number: u32, imap: &mut ImapClient) -> Option<FileContent> {
    fetch(message_number, "BODY.PEEK[HEADER.FIELDS (FROM)]", imap)
}

fn header_to(message_number: u32, imap: &mut ImapClient) -> Option<FileContent> {
    fetch(message_number, "BODY.PEEK[HEADER.FIELDS (TO)]", imap)
}

fn header_subject(message_number: u32, imap: &mut ImapClient) -> Option<FileContent> {
    fetch(message_number, "BODY.PEEK[HEADER.FIELDS (SUBJECT)]", imap)
}

fn body(message_number: u32, imap: &mut ImapClient) -> Option<FileContent> {
    fetch(message_number, "body[text]", imap)
}

fn fetch(message_number: u32, query: &str, imap: &mut ImapClient) -> Option<FileContent> {
    let message_number = format!("{}", message_number);
    match imap.fetch(&message_number, query) {
        Ok(lines) => Some(FileContent(lines)),
        Err(e) => {
            error!("Error Fetching: {}", e);
            None
        }
    }
}

type ImapClient = Client<SslStream<TcpStream>>;

struct FileContent(Vec<String>);

impl FileContent {
    fn contains(&self, substring: &str) -> bool {
        self.0.iter().any(|s| s.contains(substring))
    }
}

fn write_all(mut file: File, content: FileContent) -> Result<(), EmailError> {
    use std::io::Write;
    for line in content.0 {
        try!( file.write_all(line.as_bytes()) );
    }
    Ok(())
}

quick_error! {
    #[derive(Debug)]
    enum EmailError {
        FailedFetch
        Io(err: ::std::io::Error) {
            from()
        }
    }
}

fn input_parameters() -> (String, String, String, String, String, usize) {
    use clap::{Arg, App};
    let matches = App::new("Ukeep download")
        .arg(Arg::with_name("host")   
                .short("h")
                .long("host")
                .value_name("HOST_NAME")
                .required(true)
                .takes_value(true))
        .arg(Arg::with_name("username")   
                .short("u")
                .long("username")
                .value_name("USERNAME")
                .required(true)
                .takes_value(true))
        .arg(Arg::with_name("password")   
                .short("p")
                .long("password")
                .value_name("PASSWORD")
                .required(true)
                .takes_value(true))
        .arg(Arg::with_name("folder")   
                .short("f")
                .long("folder")
                .value_name("EMAIL_FOLDER_NAME")
                .help("Selects email folder")
                .required(true)
                .takes_value(true))
        .arg(Arg::with_name("output")   
                .short("o")
                .long("output")
                .value_name("OUTPUT_FOLDER_NAME")
                .takes_value(true))
        .arg(Arg::with_name("starting")   
                .short("s")
                .long("starting")
                .value_name("STARTING_EMAIL_NUMBER")
                .takes_value(true))
        .get_matches();
    let host = matches.value_of("host").unwrap().to_owned();
    let username = matches.value_of("username").unwrap().to_owned();
    let password = matches.value_of("password").unwrap().to_owned();
    let folder = matches.value_of("folder").unwrap().to_owned();
    let output = matches.value_of("output").map(str::to_owned).unwrap_or(folder.clone());
    use std::str::FromStr;
    let starting_number = matches.value_of("starting").map(|s| usize::from_str(s).unwrap()).unwrap_or(0);
    (host, username, password, folder, output, starting_number)
}
