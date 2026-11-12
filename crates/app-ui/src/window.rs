//! Kit owns each window Root; native workflows address its typed content.
use gpui_kit::{self as gpui, base::Root, *};
use std::marker::PhantomData;

pub(super) struct UiWindow<V> {
    handle: AnyWindowHandle,
    content: PhantomData<fn() -> V>,
}
impl<V> Copy for UiWindow<V> {}
impl<V> Clone for UiWindow<V> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<V: 'static> UiWindow<V> {
    pub fn from_handle(handle: AnyWindowHandle, cx: &App) -> Option<Self> {
        // Inspect the Root's content type without borrowing the content itself:
        // native lifecycle recovery may already be updating that entity.
        handle
            .downcast::<Root>()?
            .read(cx)
            .ok()?
            .view()
            .clone()
            .downcast::<V>()
            .ok()?;
        Some(Self {
            handle,
            content: PhantomData,
        })
    }
    pub fn from_window(window: &Window) -> Self {
        Self {
            handle: window.window_handle(),
            content: PhantomData,
        }
    }
    pub fn update<C: AppContext, R>(
        self,
        cx: &mut C,
        update: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R,
    ) -> anyhow::Result<R> {
        self.handle.update(cx, |root, window, cx| {
            let root = root
                .downcast::<Root>()
                .map_err(|_| anyhow::anyhow!("Window Root changed"))?;
            let content = root
                .read(cx)
                .view()
                .clone()
                .downcast::<V>()
                .map_err(|_| anyhow::anyhow!("Window content changed"))?;
            Ok(content.update(cx, |content, cx| update(content, window, cx)))
        })?
    }
    pub fn read(self, cx: &App) -> anyhow::Result<&V> {
        let root = self
            .handle
            .downcast::<Root>()
            .ok_or_else(|| anyhow::anyhow!("Window Root changed"))?;
        let content = root
            .read(cx)?
            .view()
            .clone()
            .downcast::<V>()
            .map_err(|_| anyhow::anyhow!("Window content changed"))?;
        Ok(content.read(cx))
    }
}
impl<V> From<UiWindow<V>> for AnyWindowHandle {
    fn from(window: UiWindow<V>) -> Self {
        window.handle
    }
}

pub(super) trait OpenKitWindow {
    fn open_kit_window<V: Render>(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> anyhow::Result<UiWindow<V>>;
}
impl OpenKitWindow for App {
    fn open_kit_window<V: Render>(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> anyhow::Result<UiWindow<V>> {
        crate::theme::init(self);
        let transparent = options.window_background == WindowBackgroundAppearance::Transparent;
        let (handle, _) = gpui::open_window(options, self, build)?;
        if transparent {
            handle.update(self, |_, window, cx| {
                Root::update(window, cx, |root, _, _| {
                    // Kit's normal surface is opaque; capture overlays must keep
                    // their native alpha background instead of covering it.
                    root.style().background = Some(rgba(0).into());
                });
            })?;
        }
        Ok(UiWindow {
            handle,
            content: PhantomData,
        })
    }
}
