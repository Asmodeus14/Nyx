use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

#[derive(Clone)]
pub struct File {
    pub id: usize,
    pub name: String,
    pub data: Vec<u8>, // Must be public!
}

pub struct FileSystem {
    pub files: Vec<File>,
    pub next_id: usize,
}

impl FileSystem {
    pub const fn new() -> Self {
        Self {
            files: Vec::new(),
            next_id: 1,
        }
    }

    pub fn init(&mut self) {
        self.create_file("readme.txt", b"Welcome to NyxOS!");
        self.create_file("config.sys", b"boot=true");
    }

    pub fn create_file(&mut self, name: &str, data: &[u8]) -> usize {
        // Overwrite if exists
        if let Some(idx) = self.files.iter().position(|f| f.name == name) {
            self.files[idx].data = data.to_vec();
            return self.files[idx].id;
        }

        // Create new
        let file = File {
            id: self.next_id,
            name: String::from(name),
            data: data.to_vec(),
        };
        self.files.push(file);
        self.next_id += 1;
        self.next_id - 1
    }

    pub fn get_file_by_id(&self, id: usize) -> Option<&File> {
        self.files.iter().find(|f| f.id == id)
    }

    pub fn get_file_by_name(&self, name: &str) -> Option<&File> {
        self.files.iter().find(|f| f.name == name)
    }
}

pub static FS: Mutex<FileSystem> = Mutex::new(FileSystem::new());