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

fn main() {
    env_logger::init().unwrap();
    let (host, username, password, email_folder, output_folder) = input_parameters();

    let mut imap_socket = imap_client(&host, &username, &password);
    let inbox = imap_socket.select(&email_folder).unwrap();
    debug!("{}", &inbox);
    let total_messages = inbox.exists;

    let saver = EmailSaver::new(&output_folder);

    for i in 1..total_messages + 1 {
        debug!("Loading email {}", i);
        match (
            header_from(i, &mut imap_socket), 
            header_to(i, &mut imap_socket),
            header_subject(i, &mut imap_socket),
            body(i, &mut imap_socket)
        ) {
            (Some(from), Some(to), Some(subject), Some(body)) => {
                saver.save(Email{
                    number: i,
                    from: from,
                    to: to,
                    subject: subject,
                    body: body
                });
            },
            (from, to, subject, body) => error!(
                "fail: from={}, to={}, subject={}, body={}", 
                from.is_some(), 
                to.is_some(), 
                subject.is_some(),
                body.is_some()
            )
        }
    }

    imap_socket.logout().unwrap();
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
        Io(err: ::std::io::Error) {
            from()
        }
    }
}

fn input_parameters() -> (String, String, String, String, String) {
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
        .get_matches();
    let host = matches.value_of("host").unwrap().to_owned();
    let username = matches.value_of("username").unwrap().to_owned();
    let password = matches.value_of("password").unwrap().to_owned();
    let folder = matches.value_of("folder").unwrap().to_owned();
    let output = matches.value_of("output").map(str::to_owned).unwrap_or(folder.clone());
    (host, username, password, folder, output)
}
