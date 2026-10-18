//! Diagnose für die Ablage beim Drag & Drop:
//! `folder_at <x> <y>`        – Ordner an einer Bildschirmposition
//! `folder_at --hwnd <handle>` – aktueller Ordner eines Explorer-Fensters

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    qross_platform::init_process();
    let folder = match args.as_slice() {
        [flag, hwnd] if flag == "--hwnd" => explorer(hwnd.parse().expect("Handle als Zahl")),
        [x, y] => qross_platform::folder_at(x.parse().expect("x"), y.parse().expect("y")),
        _ => {
            eprintln!("Aufruf: folder_at <x> <y> | folder_at --hwnd <handle>");
            std::process::exit(2);
        }
    };
    match folder {
        Some(path) => println!("{}", path.display()),
        None => println!("(kein Ordner)"),
    }
}

#[cfg(windows)]
fn explorer(hwnd: isize) -> Option<std::path::PathBuf> {
    qross_platform::explorer_folder(hwnd)
}

#[cfg(not(windows))]
fn explorer(_hwnd: isize) -> Option<std::path::PathBuf> {
    None
}
