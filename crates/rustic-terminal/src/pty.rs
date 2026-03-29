use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub type SessionId = u64;

pub struct PtySession {
    pub id: SessionId,
    pub label: String,
    pub cwd: PathBuf,
    pub is_agent: bool,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    // reader is taken out via take_reader() for the output streaming thread
    reader: Option<Box<dyn Read + Send>>,
}

impl PtySession {
    pub fn new(cwd: PathBuf, label: String, is_agent: bool, shell_program: Option<String>) -> Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Build shell command — use specified shell or system default
        let mut cmd = match shell_program {
            Some(ref prog) => CommandBuilder::new(prog),
            None => CommandBuilder::new_default_prog(),
        };
        cmd.cwd(&cwd);

        // Spawn child process
        let _child = pair.slave.spawn_command(cmd)?;
        // Drop the slave side — we communicate through the master
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;

        Ok(Self {
            id,
            label,
            cwd,
            is_agent,
            master: pair.master,
            writer,
            reader: Some(reader),
        })
    }

    /// Take the reader out for spawning a background output thread.
    /// Can only be called once.
    pub fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }
}
