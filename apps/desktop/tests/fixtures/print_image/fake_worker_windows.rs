//! Standalone owned mock. Compile explicitly on Windows; never calls a printer API.
use std::{
    ffi::c_void,
    fs::File,
    io::{Read, Write},
    os::windows::io::FromRawHandle,
};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetNamedPipeServerProcessId(pipe: *mut c_void, process: *mut u32) -> i32;
}
fn main() {
    let mode = std::env::args().nth(1).expect("owned fixture mode");
    assert_ne!(mode, "__forbidden_child", "job allowed a second process");
    let encoded = std::env::var("SCREENSHOTS_NATIVE_CAPTURE_PIPE").expect("owned test channel");
    let parts: Vec<_> = encoded.split(':').collect();
    assert!(parts.len() == 5 && parts[0] == "v1");
    let parent: u32 = parts[3].parse().unwrap();
    let mut handles = [std::ptr::null_mut(); 2];
    for (index, part) in parts[1..3].iter().enumerate() {
        handles[index] = part.parse::<usize>().unwrap() as *mut c_void;
        let mut actual = 0;
        assert_ne!(
            unsafe { GetNamedPipeServerProcessId(handles[index], &mut actual) },
            0
        );
        assert_eq!(parent, actual);
    }
    let mut input = unsafe { File::from_raw_handle(handles[0]) };
    let mut output = unsafe { File::from_raw_handle(handles[1]) };
    let hex: String = parts[4]
        .chars()
        .filter(|character| *character != '-')
        .collect();
    assert_eq!(hex.len(), 32);
    let mut expected = b"PRINTv1\n".to_vec();
    for index in (0..32).step_by(2) {
        expected.push(u8::from_str_radix(&hex[index..index + 2], 16).unwrap());
    }
    let mut handshake = [0; 24];
    input.read_exact(&mut handshake).unwrap();
    assert_eq!(handshake.as_slice(), expected);
    match mode.as_str() {
        "accept" => output.write_all(b"ACCEPT1\n").unwrap(),
        "reject" => std::process::exit(2),
        "unknown" => std::process::exit(3),
        "missing-ack" => (),
        "wrong-ack" => output.write_all(b"WRONG!!\n").unwrap(),
        "attempt-child" => {
            assert!(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .arg("__forbidden_child")
                    .spawn()
                    .is_err()
            );
            output.write_all(b"ACCEPT1\n").unwrap();
        }
        "hang" => loop {
            std::thread::park();
        },
        _ => panic!("unknown owned fixture mode"),
    }
}
