use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::Path;

const WL_DISPLAY_SYNC: u16 = 0;
const WL_DISPLAY_GET_REGISTRY: u16 = 1;

const DISPLAY_ID: u32 = 1;

struct WaylandClient {
    stream: File,
    objects: HashMap<u32, String>,
    next_id: u32,
    registry_interfaces: HashMap<u32, (String, u32)>,
}

impl WaylandClient {
    fn new(stream: File) -> Self {
        let mut objects = HashMap::new();
        objects.insert(DISPLAY_ID, "wl_display".to_string());

        Self {
            stream,
            objects,
            next_id: 2,
            registry_interfaces: HashMap::new(),
        }
    }

    fn next_object_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send_sync(&mut self) -> io::Result<u32> {
        let callback_id = self.next_object_id();

        let mut msg = vec![
            DISPLAY_ID.to_ne_bytes()[0],
            DISPLAY_ID.to_ne_bytes()[1],
            DISPLAY_ID.to_ne_bytes()[2],
            DISPLAY_ID.to_ne_bytes()[3],
            12,
            0,
            0,
            0,
            callback_id.to_ne_bytes()[0],
            callback_id.to_ne_bytes()[1],
            callback_id.to_ne_bytes()[2],
            callback_id.to_ne_bytes()[3],
        ];

        self.stream.write_all(&msg)?;
        self.objects.insert(callback_id, "wl_callback".to_string());

        Ok(callback_id)
    }

    fn get_registry(&mut self) -> io::Result<u32> {
        let registry_id = self.next_object_id();

        let mut msg = vec![
            DISPLAY_ID.to_ne_bytes()[0],
            DISPLAY_ID.to_ne_bytes()[1],
            DISPLAY_ID.to_ne_bytes()[2],
            DISPLAY_ID.to_ne_bytes()[3],
            (12 | (WL_DISPLAY_GET_REGISTRY as u32) << 16).to_ne_bytes()[0],
            (12 | (WL_DISPLAY_GET_REGISTRY as u32) << 16).to_ne_bytes()[1],
            (12 | (WL_DISPLAY_GET_REGISTRY as u32) << 16).to_ne_bytes()[2],
            (12 | (WL_DISPLAY_GET_REGISTRY as u32) << 16).to_ne_bytes()[3],
            registry_id.to_ne_bytes()[0],
            registry_id.to_ne_bytes()[1],
            registry_id.to_ne_bytes()[2],
            registry_id.to_ne_bytes()[3],
        ];

        self.stream.write_all(&msg)?;
        self.objects.insert(registry_id, "wl_registry".to_string());

        Ok(registry_id)
    }

    fn process_message(&mut self) -> io::Result<bool> {
        let mut header = [0u8; 8];
        if let Err(e) = self.stream.read_exact(&mut header) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(false);
            }
            return Err(e);
        }

        let obj_id = u32::from_ne_bytes([header[0], header[1], header[2], header[3]]);
        let size_opcode = u32::from_ne_bytes([header[4], header[5], header[6], header[7]]);
        let size = size_opcode >> 16;
        let opcode = (size_opcode & 0xFFFF) as u16;

        let body_size = size as usize - 8;
        let mut body = vec![0u8; body_size];
        self.stream.read_exact(&mut body)?;

        let obj_type = self.objects.get(&obj_id).cloned();

        match obj_type.as_deref() {
            Some("wl_registry") => {
                if opcode == 0 {
                    let name = u32::from_ne_bytes([body[0], body[1], body[2], body[3]]);

                    let mut interface_end = 4;
                    while interface_end < body.len() && body[interface_end] != 0 {
                        interface_end += 1;
                    }

                    let interface = String::from_utf8_lossy(&body[4..interface_end]).to_string();

                    let version_start = (interface_end + 4) & !3;
                    let version = if version_start + 4 <= body.len() {
                        u32::from_ne_bytes([
                            body[version_start],
                            body[version_start + 1],
                            body[version_start + 2],
                            body[version_start + 3],
                        ])
                    } else {
                        0
                    };

                    self.registry_interfaces
                        .insert(name, (interface.clone(), version));
                    println!("Global: {}(name: {}, ver: {})", interface, name, version);
                }
            }
            Some("wl_callback") => {
                if opcode == 0 {
                    println!("Sync callback received");
                }
            }
            _ => {
                println!("Unknown object: id={}, opcode={}", obj_id, opcode);
            }
        }

        Ok(true)
    }

    fn print_info(&self) {
        println!("\nWayland Server Info:");
        println!("--------------------");
        println!("Available interfaces:");

        for (name, (interface, version)) in &self.registry_interfaces {
            println!("  {} (名前: {}, バージョン: {})", interface, name, version);
        }

        println!("\nTotal interfaces: {}", self.registry_interfaces.len());
    }
}

fn from_syscall_error(error: syscall::Error) -> io::Error {
    io::Error::from_raw_os_error(error.errno as i32)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let xdg_runtime_dir =
        env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp/redox-wayland-99".to_string());

    let socket_name = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
    let socket_path = Path::new(&xdg_runtime_dir).join(&socket_name);

    println!("Connecting to Wayland compositor at {:?}", socket_path);

    let scheme_path = format!("/scheme/chan{}", socket_path.to_string_lossy());

    let client_fd = syscall::open(&scheme_path, syscall::O_RDWR).map_err(from_syscall_error)?;

    let stream = unsafe { File::from_raw_fd(client_fd as RawFd) };
    let mut client = WaylandClient::new(stream);

    println!("Connected to Wayland server");

    let _callback_id = client.send_sync()?;

    let _registry_id = client.get_registry()?;

    let mut count = 0;
    while count < 20 {
        if !client.process_message()? {
            println!("Server closed connection");
            break;
        }
        count += 1;
    }

    client.print_info();

    Ok(())
}
