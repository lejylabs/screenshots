//! Checked XGrabKey requests on one worker-owned connection; no polling backend.
use crate::hotkey::ShortcutManager;
use anyhow::{Result, ensure};
use global_hotkey::hotkey::{HotKey, Modifiers};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashSet},
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixStream,
    },
    sync::Arc,
};
use x11rb::{
    connection::Connection,
    protocol::{
        Event, xkb,
        xproto::{ConnectionExt, GrabMode, ModMask},
    },
    rust_connection::RustConnection,
};

fn keyboard_state(state: u16, locks: u16) -> u16 {
    state & 0xff & !locks
}

#[path = "unix_desktop_services_x11_stream.rs"]
mod transport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Binding {
    code: u8,
    modifiers: u16,
}
fn resolved_modifiers(modifiers: Modifiers, alt: u16, super_key: u16, locks: u16) -> Result<u16> {
    let mut result = 0;
    for (logical, physical) in [
        (Modifiers::CONTROL, 4),
        (Modifiers::SHIFT, 1),
        (Modifiers::ALT, alt),
        (Modifiers::META, super_key),
    ] {
        if !modifiers.contains(logical) {
            continue;
        }
        ensure!(
            physical != 0,
            "Configured modifier is unavailable in this X11 keymap"
        );
        ensure!(
            result & physical == 0,
            "Configured modifiers collapse to the same X11 mask"
        );
        result |= physical;
    }
    ensure!(
        result & locks == 0,
        "Shortcut modifier overlaps a lock modifier"
    );
    Ok(result)
}
fn physical_collision(first: Binding, second: Binding) -> bool {
    first.code == second.code && first.modifiers == second.modifiers
}

pub struct Registrar {
    connection: RustConnection<transport::NativeStream>,
    roots: Vec<u32>,
    alt_mask: u16,
    super_mask: u16,
    first_code: u8,
    symbols_per_code: usize,
    symbols: Vec<u32>,
    locks: u16,
    variants: Vec<u16>,
    bindings: RefCell<BTreeMap<u32, Binding>>,
    down: RefCell<HashSet<u8>>,
    poisoned: Cell<bool>,
}
impl Registrar {
    pub fn open(display: &str) -> Result<Self> {
        Self::open_native(display, None, Arc::new(|| false))
    }
    /// The control socket must be nonblocking and remain valid for this native owner.
    pub fn open_with_control(
        display: &str,
        control: UnixStream,
        canceled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self> {
        control.set_nonblocking(true)?;
        Self::open_native(display, Some(control), canceled)
    }
    fn open_native(
        display: &str,
        control: Option<UnixStream>,
        canceled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self> {
        let (stream, screen, auth_name, auth_data) =
            transport::NativeStream::connect(display, control, canceled)?;
        let connection =
            RustConnection::connect_to_stream_with_auth_info(stream, screen, auth_name, auth_data)?;
        let extension = xkb::ConnectionExt::xkb_use_extension(&connection, 1, 0)?.reply()?;
        ensure!(extension.supported, "XKB is unavailable");
        let repeat = xkb::ConnectionExt::xkb_per_client_flags(
            &connection,
            xkb::ID::USE_CORE_KBD.into(),
            xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
            xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
            Default::default(),
            Default::default(),
            Default::default(),
        )?
        .reply()?;
        ensure!(
            repeat
                .value
                .contains(xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT),
            "X11 requires detectable autorepeat"
        );
        let setup = connection.setup();
        ensure!(
            (1..=32).contains(&setup.roots.len()),
            "X11 screen count exceeds its limit"
        );
        let roots = setup
            .roots
            .iter()
            .map(|screen| screen.root)
            .collect::<Vec<_>>();
        let first_code = setup.min_keycode;
        let count = setup
            .max_keycode
            .checked_sub(first_code)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| anyhow::anyhow!("Invalid X11 keycode range"))?;
        let mapping = connection
            .get_keyboard_mapping(first_code, count)?
            .reply()?;
        let columns = usize::from(mapping.keysyms_per_keycode);
        ensure!(
            columns > 0 && mapping.keysyms.len() == usize::from(count) * columns,
            "Invalid X11 keyboard mapping"
        );
        let modifier_map = connection.get_modifier_mapping()?.reply()?;
        let per_modifier = modifier_map.keycodes.len() / 8;
        ensure!(
            per_modifier > 0 && per_modifier * 8 == modifier_map.keycodes.len(),
            "Invalid X11 modifier map"
        );
        let modifier = |wanted: &[u32]| -> Result<u16> {
            let mut masks = HashSet::new();
            for (index, row) in mapping.keysyms.chunks_exact(columns).enumerate() {
                if !row.first().is_some_and(|symbol| wanted.contains(symbol)) {
                    continue;
                }
                let code = first_code as usize + index;
                for (bit, codes) in modifier_map.keycodes.chunks_exact(per_modifier).enumerate() {
                    if codes.iter().any(|c| usize::from(*c) == code) {
                        masks.insert(1u16 << bit);
                    }
                }
            }
            ensure!(
                masks.len() <= 1,
                "Logical shortcut modifier has ambiguous X11 mappings"
            );
            Ok(masks.into_iter().next().unwrap_or(0))
        };
        let alt_mask = modifier(&[0xffe9, 0xffea])?;
        let super_mask = modifier(&[0xffeb, 0xffec])?;
        let mut lock_bits = vec![2u16];
        for symbol in [0xffe5, 0xffe6, 0xff7f, 0xff14] {
            // Num Lock and Scroll Lock need their actual keymap bits.
            for (index, row) in mapping.keysyms.chunks_exact(columns).enumerate() {
                if !row.contains(&symbol) {
                    continue;
                }
                let code = first_code as usize + index;
                for (modifier, codes) in
                    modifier_map.keycodes.chunks_exact(per_modifier).enumerate()
                {
                    if codes.iter().any(|c| usize::from(*c) == code) {
                        lock_bits.push(1 << modifier);
                    }
                }
            }
        }
        lock_bits.sort_unstable();
        lock_bits.dedup();
        ensure!(lock_bits.len() <= 3, "Unsupported X11 lock modifier map");
        let mut locks = 0;
        let mut variants = vec![0];
        for bit in lock_bits {
            locks |= bit;
            let extra: Vec<_> = variants.iter().map(|v| *v | bit).collect();
            variants.extend(extra);
        }
        variants.sort_unstable();
        variants.dedup();
        let fd = connection.stream().as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        ensure!(
            flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == 0,
            "Could not protect the shortcut connection from child inheritance"
        );
        connection.stream().ready()?;
        Ok(Self {
            connection,
            roots,
            alt_mask,
            super_mask,
            first_code,
            symbols_per_code: columns,
            symbols: mapping.keysyms,
            locks,
            variants,
            bindings: RefCell::new(BTreeMap::new()),
            down: RefCell::new(HashSet::new()),
            poisoned: Cell::new(false),
        })
    }
    pub fn register_shortcut(&self, shortcut: &str) -> Result<u32> {
        let settings = crate::CaptureHotkeys([
            shortcut.into(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ])
        .validated()?;
        let key: HotKey = settings.0[0].parse()?;
        ShortcutManager::register(self, key)?;
        Ok(key.id())
    }
    pub fn unregister_shortcut(&self, shortcut: &str) -> Result<()> {
        ShortcutManager::unregister(self, shortcut.parse()?)
    }
    pub(super) fn poisoned(&self) -> bool {
        self.poisoned.get() || self.connection.stream().failed()
    }
    pub fn fd(&self) -> RawFd {
        self.connection.stream().as_raw_fd()
    }
    pub fn drain(&self) -> Result<(Vec<u32>, bool)> {
        let _operation = self.connection.stream().begin()?;
        let mut pressed = Vec::new();
        // The server may flood events; bound one dispatch batch before checking shutdown/control.
        let mut full = true;
        for _ in 0..64 {
            let Some(event) = self.connection.poll_for_event()? else {
                full = false;
                break;
            };
            match event {
                Event::KeyRelease(event) => {
                    self.down.borrow_mut().remove(&event.detail);
                }
                Event::KeyPress(event) => {
                    let state = keyboard_state(u16::from(event.state), self.locks);
                    let bindings = self.bindings.borrow();
                    if let Some((id, _)) = bindings
                        .iter()
                        .find(|(_, b)| b.code == event.detail && b.modifiers == state)
                    {
                        if self.down.borrow_mut().insert(event.detail) {
                            pressed.push(*id);
                        }
                    }
                }
                Event::MappingNotify(_) => {
                    anyhow::bail!("Keyboard mapping changed; restart to register shortcuts safely")
                }
                Event::Error(error) => {
                    anyhow::bail!("X11 shortcut connection failed: {:?}", error.error_kind)
                }
                _ => {}
            }
        }
        Ok((pressed, full))
    }
    fn binding(&self, key: HotKey) -> Result<Binding> {
        let name = key.key.to_string();
        let symbol = if let Some(n) = name
            .strip_prefix('F')
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|n| (1..=24).contains(n))
        {
            0xffbd + n
        } else if name == "PrintScreen" {
            0xff61
        } else if let Some(n) = name.strip_prefix("Key").filter(|s| s.len() == 1) {
            u32::from(n.as_bytes()[0].to_ascii_lowercase())
        } else if let Some(n) = name.strip_prefix("Digit").filter(|s| s.len() == 1) {
            u32::from(n.as_bytes()[0])
        } else {
            anyhow::bail!("Unsupported X11 shortcut key")
        };
        let mut codes = self
            .symbols
            .chunks_exact(self.symbols_per_code)
            .enumerate()
            .filter(|(_, row)| row.first() == Some(&symbol))
            .map(|(i, _)| self.first_code as usize + i);
        let code = codes.next().ok_or_else(|| {
            anyhow::anyhow!("Configured shortcut is absent from the X11 base keymap")
        })?;
        ensure!(
            codes.next().is_none(),
            "Configured shortcut is ambiguous in the X11 keymap"
        );
        let modifiers = resolved_modifiers(key.mods, self.alt_mask, self.super_mask, self.locks)?;
        Ok(Binding {
            code: u8::try_from(code)?,
            modifiers,
        })
    }
}
impl ShortcutManager for Registrar {
    fn register(&self, key: HotKey) -> Result<()> {
        let _operation = self.connection.stream().begin()?;
        ensure!(
            !self.poisoned(),
            "Native registration cleanup failed; restart required"
        );
        ensure!(
            self.bindings.borrow().len() < 16,
            "Native shortcut reservation limit reached"
        );
        ensure!(
            !self.bindings.borrow().contains_key(&key.id()),
            "Shortcut is already reserved"
        );
        let binding = self.binding(key)?;
        ensure!(
            !self
                .bindings
                .borrow()
                .values()
                .any(|held| physical_collision(binding, *held)),
            "Configured shortcut resolves to an already reserved X11 chord"
        );
        let mut grabbed = Vec::new();
        let result = (|| -> Result<()> {
            for root in &self.roots {
                for lock in &self.variants {
                    let mask = binding.modifiers | lock;
                    self.connection
                        .grab_key(
                            false,
                            *root,
                            ModMask::from(mask),
                            binding.code,
                            GrabMode::ASYNC,
                            GrabMode::ASYNC,
                        )?
                        .check()?;
                    grabbed.push((*root, mask));
                }
            }
            self.connection.flush()?;
            Ok(())
        })();
        if let Err(error) = result {
            for (root, previous) in grabbed {
                let cleanup = (|| -> Result<()> {
                    self.connection
                        .ungrab_key(binding.code, root, ModMask::from(previous))?
                        .check()?;
                    Ok(())
                })();
                if cleanup.is_err() {
                    self.poisoned.set(true);
                }
            }
            if self.poisoned.get() {
                anyhow::bail!(
                    "Shortcut reservation failed and cleanup could not be confirmed; restart required"
                );
            }
            anyhow::bail!("Shortcut is unavailable or conflicted: {error:#}");
        }
        self.bindings.borrow_mut().insert(key.id(), binding);
        Ok(())
    }
    fn unregister(&self, key: HotKey) -> Result<()> {
        let _operation = self.connection.stream().begin()?;
        let bindings = self.bindings.borrow();
        let binding = bindings
            .get(&key.id())
            .ok_or_else(|| anyhow::anyhow!("Unknown shortcut reservation"))?;
        for root in &self.roots {
            for lock in &self.variants {
                self.connection
                    .ungrab_key(binding.code, *root, ModMask::from(binding.modifiers | lock))?
                    .check()?;
            }
        }
        let code = binding.code;
        drop(bindings);
        self.bindings.borrow_mut().remove(&key.id());
        self.down.borrow_mut().remove(&code);
        self.connection.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliased_alt_super_chords_collide_before_any_grab() -> Result<()> {
        let alt = Binding {
            code: 42,
            modifiers: resolved_modifiers(Modifiers::ALT, 8, 8, 2)?,
        };
        let super_key = Binding {
            code: 42,
            modifiers: resolved_modifiers(Modifiers::META, 8, 8, 2)?,
        };
        assert!(physical_collision(alt, super_key));
        assert!(!physical_collision(
            alt,
            Binding {
                code: 43,
                ..super_key
            }
        ));
        assert!(resolved_modifiers(Modifiers::ALT | Modifiers::META, 8, 8, 2).is_err());
        assert!(resolved_modifiers(Modifiers::CONTROL | Modifiers::ALT, 4, 64, 2).is_err());
        assert!(resolved_modifiers(Modifiers::SHIFT | Modifiers::META, 8, 1, 2).is_err());
        Ok(())
    }
    #[test]
    fn pointer_buttons_and_xkb_group_bits_do_not_change_keyboard_chords() {
        assert_eq!(
            super::keyboard_state(4 | 1 | 2 | 16 | (1 << 8) | (1 << 13), 2 | 16),
            4 | 1
        );
    }
}
