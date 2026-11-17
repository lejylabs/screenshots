//! Only private in-process COM and HGLOBAL objects, with no OLE/user session.
use super::*;
use core::prelude::v1::test;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc};

#[implement(ISequentialStream)]
struct MediumOwner {
    _memory: Owned<HGLOBAL>,
    releases: Arc<AtomicUsize>,
    on_release: Option<Box<dyn Fn()>>,
}

impl Drop for MediumOwner {
    fn drop(&mut self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
        if let Some(on_release) = &self.on_release {
            on_release();
        }
    }
}

#[allow(non_snake_case)]
impl ISequentialStream_Impl for MediumOwner_Impl {
    fn Read(&self, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> HRESULT {
        E_NOTIMPL
    }

    fn Write(&self, _: *const core::ffi::c_void, _: u32, _: *mut u32) -> HRESULT {
        E_NOTIMPL
    }
}

#[test]
fn acquired_medium_releases_its_provider_once_after_rejected_metadata() {
    let releases = Arc::new(AtomicUsize::new(0));
    let memory = unsafe { Owned::new(GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 1).unwrap()) };
    let handle = *memory;
    let provider: IUnknown = {
        let stream: ISequentialStream = MediumOwner {
            _memory: memory,
            releases: releases.clone(),
            on_release: None,
        }
        .into();
        stream.cast().unwrap()
    };
    let read = || {
        let medium = FileDropMedium(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: handle },
            pUnkForRelease: std::mem::ManuallyDrop::new(Some(provider)),
        });
        read_file_paths(unsafe { medium.0.u.hGlobal })
    };
    assert!(read().is_none());
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn medium_without_provider_releases_its_private_hglobal() {
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, 32).unwrap() };
    {
        let _medium = FileDropMedium(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: handle },
            ..Default::default()
        });
    }
    assert_eq!(
        unsafe { windows::Win32::System::Memory::GlobalSize(handle) },
        0
    );
}

#[test]
fn medium_release_reentrant_enter_supersedes_old_generation_exactly_once() {
    let state = Rc::new(FileDropState::default());
    let old = state.begin_enter(|| {}, || {}).unwrap();
    let releases = Arc::new(AtomicUsize::new(0));
    let memory = unsafe { Owned::new(GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, 1).unwrap()) };
    let handle = *memory;
    let provider: IUnknown = {
        let state = state.clone();
        let stream: ISequentialStream = MediumOwner {
            _memory: memory,
            releases: releases.clone(),
            on_release: Some(Box::new(move || {
                let newer = state.begin_enter(|| {}, || {}).unwrap();
                assert!(state.helper_enter(newer));
                assert!(state.accept(newer));
            })),
        }
        .into();
        stream.cast().unwrap()
    };
    {
        let medium = FileDropMedium(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: handle },
            pUnkForRelease: std::mem::ManuallyDrop::new(Some(provider)),
        });
        assert!(read_file_paths(unsafe { medium.0.u.hGlobal }).is_none());
    }
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    assert!(!state.is_current(old));
    assert!(state.accepted().is_some());
    assert!(state.helper_is_open());
    let terminals = Cell::new(0);
    assert!(state.finish_drop(
        |accepted| {
            assert!(accepted);
            terminals.set(terminals.get() + 1);
        },
        |accepted| accepted
    ));
    assert_eq!(terminals.get(), 1);
    assert!(!state.helper_is_open());
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}
