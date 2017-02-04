# ukeep

#Использование

cargo build --release && RUST_LOG=ukeep RUST_BACKTRACE=1 cargo run --release -- --host "imap.yandex.ru" --username "username" --password "password" --folder "INBOX"

