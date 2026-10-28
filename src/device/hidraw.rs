use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gettextrs::gettext;

use crate::i18n::{msgid, tf};
use crate::model::Model;
use crate::protocol::{REPORT_ID, WIRED_BUFFER_LEN, WIRED_REPORT_LEN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Wireless,
    Wired,
    Simulated,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wireless => msgid("2.4G 无线接收器"),
            Self::Wired => msgid("USB 有线"),
            Self::Simulated => msgid("模拟设备"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub product_name: String,
    pub hidraw: String,
    pub path: PathBuf,
    pub transport: Transport,
}

#[derive(Debug)]
pub enum DeviceError {
    NotFound,
    Permission { path: PathBuf },
    Io(io::Error),
    Protocol(crate::protocol::ProtocolError),
    Timeout,
    Verify(String),
    Macro(crate::macro_table::MacroError),
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::NotFound => gettext("未检测到 MCHOSE K99（USB 有线或 2.4G 接收器）"),
            Self::Permission { path } => tf(
                "无权访问 {}。请安装 udev 规则或把用户加入 input/plugdev 组。",
                &[&path.display()],
            ),
            Self::Io(error) => tf("HID 读写失败：{}", &[error]),
            Self::Protocol(error) => tf("设备协议错误：{}", &[error]),
            Self::Timeout => gettext("设备无响应，键盘可能已休眠；按任意键唤醒后点击刷新"),
            Self::Verify(message) => message.clone(),
            Self::Macro(error) => error.to_string(),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for DeviceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::Macro(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for DeviceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<crate::protocol::ProtocolError> for DeviceError {
    fn from(error: crate::protocol::ProtocolError) -> Self {
        Self::Protocol(error)
    }
}

impl From<crate::macro_table::MacroError> for DeviceError {
    fn from(error: crate::macro_table::MacroError) -> Self {
        Self::Macro(error)
    }
}

// _IOWR('H', 0x07, 520) / _IOWR('H', 0x06, 520): feature payload plus report ID.
const HIDIOCGFEATURE_520: libc::c_ulong = 0xc208_4807;
const HIDIOCSFEATURE_520: libc::c_ulong = 0xc208_4806;

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

    /// Read the wired vendor feature report; the report ID is stripped from
    /// the returned payload.
    pub fn get_feature(&mut self, report_id: u8) -> Result<Vec<u8>, DeviceError> {
        let mut buffer = [0u8; WIRED_BUFFER_LEN];
        buffer[0] = report_id;
        let rc = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                HIDIOCGFEATURE_520,
                buffer.as_mut_ptr(),
            )
        };
        if rc < 0 {
            return Err(DeviceError::Io(io::Error::last_os_error()));
        }
        Ok(buffer[1..].to_vec())
    }

    pub fn set_feature(
        &mut self,
        report_id: u8,
        payload: &[u8; WIRED_REPORT_LEN],
    ) -> Result<(), DeviceError> {
        let mut buffer = [0u8; WIRED_BUFFER_LEN];
        buffer[0] = report_id;
        buffer[1..].copy_from_slice(payload);
        let rc = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                HIDIOCSFEATURE_520,
                buffer.as_mut_ptr(),
            )
        };
        if rc < 0 {
            return Err(DeviceError::Io(io::Error::last_os_error()));
        }
        Ok(())
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

fn feature_report_length(descriptor: &[u8], report_id: u8) -> Option<usize> {
    let mut usage_page = 0u16;
    let mut current_report = 0u8;
    let mut report_count = 0u32;
    let mut report_size = 0u32;
    let mut i = 0;
    while i < descriptor.len() {
        let prefix = descriptor[i];
        i += 1;
        if prefix == 0xfe {
            if i + 2 > descriptor.len() {
                break;
            }
            let len = descriptor[i] as usize;
            i += 1 + len + 1;
            continue;
        }
        let size = match prefix & 0x03 {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        if i + size > descriptor.len() {
            break;
        }
        let data = &descriptor[i..i + size];
        i += size;
        let item_type = (prefix >> 2) & 0x03;
        let tag = (prefix >> 4) & 0x0f;
        let value = data
            .iter()
            .rev()
            .fold(0u32, |acc, byte| (acc << 8) | u32::from(*byte));
        match (item_type, tag) {
            (1, 0x0) => usage_page = value as u16,
            (1, 0x7) => report_size = value,
            (1, 0x8) => current_report = value as u8,
            (1, 0x9) => report_count = value,
            (0, 0xb) => {
                let bytes = (report_count as usize * report_size as usize).div_ceil(8);
                if usage_page == 0xff00 && current_report == report_id && bytes == WIRED_REPORT_LEN
                {
                    return Some(bytes);
                }
            }
            _ => {}
        }
    }
    None
}

fn has_wired_feature_report(hidraw: &Path, report_id: u8) -> bool {
    fs::read(hidraw.join("device/report_descriptor"))
        .ok()
        .and_then(|desc| feature_report_length(&desc, report_id))
        .is_some()
}

pub fn find_wired(model: &Model) -> Result<DeviceInfo, DeviceError> {
    let mut permission: Option<PathBuf> = None;
    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        return Err(DeviceError::NotFound);
    };
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
            || !product_name.to_ascii_uppercase().contains("MCHOSE")
        {
            continue;
        }
        if !has_wired_feature_report(&sys, model.report.wired_id) {
            continue;
        }
        let path = PathBuf::from("/dev").join(&name);
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => {
                drop(file);
                return Ok(DeviceInfo {
                    vendor_id: vid,
                    product_id: pid,
                    product_name,
                    hidraw: name,
                    path,
                    transport: Transport::Wired,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => permission = Some(path),
            Err(_) => {}
        }
    }
    match permission {
        Some(path) => Err(DeviceError::Permission { path }),
        None => Err(DeviceError::NotFound),
    }
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
                    transport: Transport::Wireless,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_vendor_feature_report() {
        let descriptor = [
            0x06, 0x00, 0xff, 0x09, 0x01, 0xa1, 0x01, 0x85, 0x06, 0x75, 0x08, 0x96, 0x07, 0x02,
            0xb1, 0x02, 0xc0,
        ];
        assert_eq!(feature_report_length(&descriptor, 6), Some(519));
        assert_eq!(feature_report_length(&descriptor, 5), None);
    }

    #[test]
    fn ignores_short_vendor_feature() {
        let descriptor = [
            0x06, 0x00, 0xff, 0x85, 0x05, 0x75, 0x08, 0x95, 0x05, 0xb1, 0x02,
        ];
        assert_eq!(feature_report_length(&descriptor, 5), None);
    }
}
