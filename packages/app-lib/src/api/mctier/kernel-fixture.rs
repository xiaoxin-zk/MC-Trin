use std::{
    io,
    net::{TcpListener, TcpStream},
    path::PathBuf,
    thread,
    time::Duration,
};

fn argument(name: &str, arguments: &[String]) -> Option<String> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let root = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::write(root.join("arguments"), arguments.join("\n")).unwrap();
    if arguments.iter().any(|argument| argument == "--help") {
        println!("--no-tun --port-forward");
        return;
    }
    if root.join("delay").exists() {
        thread::sleep(Duration::from_secs(30));
    }
    let listener =
        TcpListener::bind(argument("--rpc-portal", &arguments).unwrap())
            .unwrap();
    if let Some(rule) = argument("--port-forward", &arguments) {
        let (bind, destination) = rule
            .strip_prefix("tcp://")
            .unwrap()
            .split_once('/')
            .unwrap();
        let port = destination.rsplit_once(':').unwrap().1.to_string();
        let forwarded = TcpListener::bind(bind).unwrap();
        thread::spawn(move || {
            for stream in forwarded.incoming().flatten() {
                let port = port.clone();
                thread::spawn(move || {
                    let Ok(destination) =
                        TcpStream::connect(format!("127.0.0.1:{port}"))
                    else {
                        return;
                    };
                    let (mut incoming, mut outgoing) = (
                        stream.try_clone().unwrap(),
                        destination.try_clone().unwrap(),
                    );
                    let task = thread::spawn(move || {
                        let _ = io::copy(&mut incoming, &mut outgoing);
                    });
                    let (mut destination, mut stream) = (destination, stream);
                    let _ = io::copy(&mut destination, &mut stream);
                    let _ = task.join();
                });
            }
        });
    }
    std::fs::write(
        PathBuf::from(root).join("rpc-port"),
        listener.local_addr().unwrap().port().to_string(),
    )
    .unwrap();
    for stream in listener.incoming() {
        drop(stream);
    }
}
