//! Versioned, length-delimited recordings of console-api messages (not raw tracing events).
use crate::{
    config::Settings,
    model::{Accumulator, Snapshot},
};
use anyhow::{Context, Result, bail};
use console_api::{instrument::Update, tasks::TaskDetails};
use prost::Message;
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
};

const MAGIC: &[u8; 8] = b"TCUI\x01\0\0\0";
const MAX_FRAME: usize = 16 * 1024 * 1024;
pub struct Recorder(BufWriter<File>);

impl Recorder {
    pub fn create(path: &Path) -> Result<Self> {
        let mut writer = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .with_context(|| {
                    format!(
                        "Create recording {} (existing files are never overwritten)",
                        path.display()
                    )
                })?,
        );
        writer.write_all(MAGIC)?;
        writer.flush()?;
        Ok(Self(writer))
    }
    fn frame(&mut self, kind: u8, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_FRAME {
            bail!("Recording frame exceeds 16 MiB");
        }
        self.0.write_all(&[kind])?;
        self.0.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.0.write_all(bytes)?;
        self.0.flush()?;
        Ok(())
    }
    pub fn reset(&mut self) -> Result<()> {
        self.frame(3, &[])
    }
    pub fn update(&mut self, update: &Update) -> Result<()> {
        self.frame(1, &update.encode_to_vec())
    }
    pub fn details(&mut self, details: &TaskDetails) -> Result<()> {
        self.frame(2, &details.encode_to_vec())
    }
}

pub fn replay(path: &Path, settings: &Settings) -> Result<Snapshot> {
    let mut reader =
        BufReader::new(File::open(path).with_context(|| format!("Open {}", path.display()))?);
    let mut header = [0; 8];
    reader
        .read_exact(&mut header)
        .context("Truncated recording header")?;
    if &header != MAGIC {
        bail!(
            "Not a Console UI v1 recording. console-subscriber raw tracing recordings use a different format."
        );
    }
    let mut accumulator = Accumulator::default();
    loop {
        let mut kind = [0];
        if reader.read(&mut kind)? == 0 {
            break;
        }
        let mut length = [0; 4];
        reader
            .read_exact(&mut length)
            .context("Truncated recording frame length")?;
        let length = u32::from_le_bytes(length) as usize;
        if length > MAX_FRAME {
            bail!("Recording frame exceeds 16 MiB");
        }
        let mut bytes = vec![0; length];
        reader
            .read_exact(&mut bytes)
            .context("Truncated recording frame payload")?;
        match kind[0] {
            1 => {
                accumulator.apply(Update::decode(bytes.as_slice())?);
                accumulator.prune(settings.retention()?);
            }
            2 => accumulator.apply_details(TaskDetails::decode(bytes.as_slice())?),
            3 if length == 0 => accumulator = Accumulator::default(),
            tag => bail!("Unknown recording frame type {tag}"),
        }
    }
    Ok(accumulator.snapshot(&settings.warnings))
}
