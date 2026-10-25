use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use crate::model::Model;
use crate::protocol::REPORT_ID;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub product_name: String,
    pub hidraw: String,
    pub path: PathBuf,
}

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("未检测到 MCHOSE 2.4G 接收器")]
    NotFound,
    #[error("无权访问 {path}。请安装 udev 规则或把用户加入 input/plugdev 组。")]
    Permission { path: PathBuf },
    #[error("HID 读写失败：{0}")]
    Io(#[from] io::Error),
    #[error("设备协议错误：{0}")]
    Protocol(#[from] crate::protocol::ProtocolError),
    #[error("设备无响应")]
    Timeout,
    #[error("{0}")]
    Verify(&'static str),
}

pub struct HidrawDevice {
    pub info: DeviceInfo,
    file: File,
}

impl HidrawDevice {
    pub fn open(info: DeviceInfo) -> Result<Self, DeviceError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&info.path)
            .map_err(|e| {
                if e.kind() == io::ErrorKind::PermissionDenied {
                    DeviceError::Permission {
                        path: info.path.clone(),
                    }
                } else {
                    DeviceError::Io(e)
                }
            })?;
        Ok(Self { info, file })
    }

    pub fn is_present(&self) -> bool {
        self.info.path.exists()
    }

    pub fn write_report(&mut self, report: &[u8; 20]) -> Result<(), DeviceError> {
        self.file.write_all(report)?;
        self.file.flush()?;
        Ok(())
    }

    pub fn read_report(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError> {
        let mut pfd = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis() as i32) };
        if rc == 0 {
            return Ok(None);
        }
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(None);
            }
            return Err(DeviceError::Io(err));
        }
        if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(DeviceError::Io(io::Error::from(
                io::ErrorKind::ConnectionAborted,
            )));
        }
        let mut buf = [0u8; 64];
        let n = unsafe {
            libc::read(
                self.file.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(DeviceError::Io(err));
        }
        Ok(Some(buf[..n as usize].to_vec()))
    }

    pub fn drain(&mut self) {
        for _ in 0..64 {
            match self.read_report(Duration::ZERO) {
                Ok(Some(_)) => continue,
                _ => break,
            }
        }
    }
}

fn read_uevent(hidraw: &Path) -> Option<(u16, u16, String)> {
    let uevent = fs::read_to_string(hidraw.join("device/uevent")).ok()?;
    let mut vid = None;
    let mut pid = None;
    let mut name = None;
    for line in uevent.lines() {
        if let Some(v) = line.strip_prefix("HID_ID=") {
            let parts: Vec<&str> = v.split(':').collect();
            if parts.len() == 3 {
                vid = u16::from_str_radix(parts[1], 16).ok();
                pid = u16::from_str_radix(parts[2], 16).ok();
            }
        } else if let Some(v) = line.strip_prefix("HID_NAME=") {
            name = Some(v.trim().to_string());
        }
    }
    Some((vid?, pid?, name?))
}

fn has_vendor_output_report(hidraw: &Path) -> bool {
    let Ok(desc) = fs::read(hidraw.join("device/report_descriptor")) else {
        return false;
    };
    let has_vendor_page = desc.windows(3).any(|w| w == [0x06, 0x02, 0xff]);
    let has_report_id = desc.windows(2).any(|w| w == [0x85, REPORT_ID]);
    has_vendor_page && has_report_id
}

pub fn find_receiver(model: &Model) -> Result<DeviceInfo, DeviceError> {
    let mut permission: Option<PathBuf> = None;
    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        return Err(DeviceError::NotFound);
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("hidraw") {
            continue;
        }
        let sys = entry.path();
        let Some((vid, pid, product_name)) = read_uevent(&sys) else {
            continue;
        };
        if vid != model.receiver.vendor_id
            || pid != model.receiver.product_id
            || !product_name.eq_ignore_ascii_case(&model.receiver.product_name)
        {
            continue;
        }
        if !has_vendor_output_report(&sys) {
            continue;
        }
        let path = PathBuf::from("/dev").join(&name);
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => {
                drop(file);
                candidates.push(DeviceInfo {
                    vendor_id: vid,
                    product_id: pid,
                    product_name: product_name.clone(),
                    hidraw: name,
                    path,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => permission = Some(path),
            Err(_) => {}
        }
    }
    if let Some(info) = candidates.into_iter().next() {
        return Ok(info);
    }
    if let Some(path) = permission {
        return Err(DeviceError::Permission { path });
    }
    Err(DeviceError::NotFound)
}
