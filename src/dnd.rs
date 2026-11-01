// OLE side of the clipboard page: reading and writing the system clipboard, and drag and drop in and out.
// Everything is a list of (format, bytes) pairs (clipboard::Format) so one data object serves both a
// clipboard write and a drag.

use crate::clipboard::{CF_DIB, CF_HDROP, CF_UNICODETEXT, Format, MAX_ITEM_BYTES};
use std::{cell::Cell, mem::ManuallyDrop};
use windows::{
    Win32::{
        Foundation::*,
        System::{
            Com::*,
            DataExchange::GetClipboardFormatNameW,
            Memory::*,
            Ole::*,
            SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
        },
        UI::Shell::SHCreateStdEnumFmtEtc,
    },
    core::*,
};

const FIRST_REGISTERED_FORMAT: u16 = 0xC000;
const HGLOBAL_MEDIUM: u32 = TYMED_HGLOBAL.0 as u32;
// Password managers set this: the copy must not be recorded anywhere.
const EXCLUDE_FORMAT: &str = "ExcludeClipboardContentFromMonitorProcessing";
const HISTORY_FORMAT: &str = "CanIncludeInClipboardHistory"; // a DWORD of 0 means the same
// Virtual files (zip entries, mail attachments) need their streams, and a remembered "cut" would turn a later
// paste into a move: neither survives being replayed from memory.
const SKIPPED_FORMATS: [&str; 4] = ["FileGroupDescriptorW", "FileGroupDescriptor", "FileContents", "Preferred DropEffect"];
const SET_RETRIES: u32 = 5; // another app can hold the clipboard open for a moment

thread_local!(static SELF_DRAG: Cell<bool> = const { Cell::new(false) }); // our own drag is in flight

fn format_name(cf: u16) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClipboardFormatNameW(cf as u32, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn query(cf: u16) -> FORMATETC {
    FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: HGLOBAL_MEDIUM }
}

fn alloc_global(bytes: &[u8]) -> Result<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

enum Read {
    Bytes(Vec<u8>),
    TooBig,
    Failed,
}

fn read_format(data: &IDataObject, f: &FORMATETC, budget: usize) -> Read {
    unsafe {
        let Ok(mut medium) = data.GetData(f) else { return Read::Failed };
        let result = if medium.tymed != HGLOBAL_MEDIUM {
            Read::Failed
        } else {
            let h = medium.u.hGlobal;
            let size = GlobalSize(h);
            let p = GlobalLock(h);
            if size > budget {
                Read::TooBig
            } else if p.is_null() {
                Read::Failed
            } else {
                Read::Bytes(std::slice::from_raw_parts(p.cast::<u8>(), size).to_vec())
            }
        };
        if medium.tymed == HGLOBAL_MEDIUM {
            let _ = GlobalUnlock(medium.u.hGlobal);
        }
        ReleaseStgMedium(&mut medium);
        result
    }
}

/// The copying app asked not to be recorded (password managers do). Checked by format name first, so a secret is
/// never even read.
fn marked_private(data: &IDataObject) -> bool {
    let Ok(formats) = (unsafe { data.EnumFormatEtc(DATADIR_GET.0 as u32) }) else { return false };
    loop {
        let mut slot = [FORMATETC::default()];
        let mut got = 0u32;
        if unsafe { formats.Next(&mut slot, Some(&mut got)) } != S_OK || got == 0 {
            return false;
        }
        let mut f = slot[0];
        if f.cfFormat < FIRST_REGISTERED_FORMAT {
            continue;
        }
        match format_name(f.cfFormat).as_str() {
            EXCLUDE_FORMAT => return true,
            HISTORY_FORMAT => {
                f.tymed = HGLOBAL_MEDIUM;
                // A DWORD of 0 means "not for history"; an empty or unreadable value says nothing.
                if let Read::Bytes(b) = read_format(data, &f, 16) {
                    if b.len() >= 4 && b[..4].iter().all(|&x| x == 0) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
}

/// The formats worth keeping from a data object (the clipboard, or something dropped on the notch).
/// None if it should not be recorded: marked private, or too big.
pub fn snapshot(data: &IDataObject) -> Option<Vec<Format>> {
    if marked_private(data) {
        return None;
    }
    let formats = unsafe { data.EnumFormatEtc(DATADIR_GET.0 as u32) }.ok()?;
    let mut out: Vec<Format> = Vec::new();
    let mut total = 0usize;
    loop {
        let mut slot = [FORMATETC::default()];
        let mut got = 0u32;
        if unsafe { formats.Next(&mut slot, Some(&mut got)) } != S_OK || got == 0 {
            break;
        }
        let mut f = slot[0];
        let offers_memory = f.tymed & HGLOBAL_MEDIUM != 0;
        f.tymed = HGLOBAL_MEDIUM; // ask for memory even if the source also offers a stream
        let cf = f.cfFormat;
        if !offers_memory || out.iter().any(|(c, _)| *c == cf) {
            continue;
        }
        let name = if cf >= FIRST_REGISTERED_FORMAT { format_name(cf) } else { String::new() };
        // Text, files and bitmaps by their standard ids; everything else registered (HTML, RTF, PNG...) rides along.
        let keep = matches!(cf, CF_DIB | CF_HDROP | CF_UNICODETEXT)
            || (cf >= FIRST_REGISTERED_FORMAT && !SKIPPED_FORMATS.contains(&name.as_str()));
        if !keep {
            continue;
        }
        match read_format(data, &f, MAX_ITEM_BYTES - total) {
            Read::Bytes(bytes) => {
                total += bytes.len();
                out.push((cf, bytes));
            }
            Read::TooBig => {} // skip just this format: the rest of the copy is still worth keeping
            Read::Failed => {}
        }
    }
    Some(out).filter(|o| !o.is_empty())
}

/// What is on the system clipboard right now. None if it is busy, empty or marked private.
pub fn clipboard_snapshot() -> Option<Vec<Format>> {
    snapshot(&unsafe { OleGetClipboard() }.ok()?)
}

/// Make `formats` the system clipboard's contents (rendered right away, so they outlive this program).
pub fn set_clipboard(formats: Vec<Format>) -> bool {
    let data: IDataObject = DataObject { formats }.into();
    for _ in 0..SET_RETRIES {
        if unsafe { OleSetClipboard(&data) }.is_ok() {
            let _ = unsafe { OleFlushClipboard() };
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    false
}

#[implement(IDataObject)]
struct DataObject {
    formats: Vec<Format>,
}

impl IDataObject_Impl for DataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> Result<STGMEDIUM> {
        let f = unsafe { &*pformatetcin };
        if f.tymed & HGLOBAL_MEDIUM == 0 {
            return Err(DV_E_TYMED.into());
        }
        let Some((_, bytes)) = self.formats.iter().find(|(cf, _)| *cf == f.cfFormat) else {
            return Err(DV_E_FORMATETC.into());
        };
        Ok(STGMEDIUM {
            tymed: HGLOBAL_MEDIUM,
            u: STGMEDIUM_0 { hGlobal: alloc_global(bytes)? },
            pUnkForRelease: ManuallyDrop::new(None),
        })
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        let f = unsafe { &*pformatetc };
        if f.tymed & HGLOBAL_MEDIUM == 0 {
            DV_E_TYMED
        } else if self.formats.iter().any(|(cf, _)| *cf == f.cfFormat) {
            S_OK
        } else {
            DV_E_FORMATETC
        }
    }

    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, pformatetcout: *mut FORMATETC) -> HRESULT {
        unsafe { (*pformatetcout).ptd = std::ptr::null_mut() };
        DATA_S_SAMEFORMATETC
    }

    fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> Result<IEnumFORMATETC> {
        if dwdirection != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let list: Vec<FORMATETC> = self.formats.iter().map(|(cf, _)| query(*cf)).collect();
        unsafe { SHCreateStdEnumFmtEtc(&list) }
    }

    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<'_, IAdviseSink>) -> Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if grfkeystate.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drag `formats` out of the notch; returns when dropped or cancelled. Copy only: the history keeps its item.
/// Must not be called while main.rs holds its APP borrow (our own drop target is called back during the drag).
pub fn drag(formats: Vec<Format>) {
    let data: IDataObject = DataObject { formats }.into();
    let source: IDropSource = DropSource.into();
    SELF_DRAG.set(true);
    let mut effect = DROPEFFECT(0);
    let _ = unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect) };
    SELF_DRAG.set(false);
}

/// The source of the drag allows copying (a few only allow move or link, which we cannot honour).
fn allows_copy(effect: *const DROPEFFECT) -> bool {
    unsafe { (*effect).0 & DROPEFFECT_COPY.0 != 0 }
}

fn accepts(data: &IDataObject) -> bool {
    [CF_HDROP, CF_UNICODETEXT, CF_DIB].iter().any(|&cf| unsafe { data.QueryGetData(&query(cf)) } == S_OK)
}

#[implement(IDropTarget)]
struct DropTarget {
    hwnd: isize,
    accepting: Cell<bool>, // the drag in progress carries something we can keep
}

impl DropTarget {
    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, pdataobj: Ref<'_, IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        // Something we dragged out of the notch itself is not dropped back in.
        let ok = !SELF_DRAG.get() && allows_copy(pdweffect) && pdataobj.as_ref().is_some_and(accepts);
        self.accepting.set(ok);
        unsafe { *pdweffect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        if ok {
            crate::clippage::drag_hover(self.hwnd(), true);
        }
        Ok(())
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        let copy = self.accepting.get() && allows_copy(pdweffect);
        unsafe { *pdweffect = if copy { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if self.accepting.replace(false) {
            crate::clippage::drag_hover(self.hwnd(), false);
        }
        Ok(())
    }

    fn Drop(&self, pdataobj: Ref<'_, IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, pdweffect: *mut DROPEFFECT) -> Result<()> {
        let mut effect = DROPEFFECT_NONE;
        if self.accepting.replace(false) {
            if let Some(formats) = pdataobj.as_ref().and_then(snapshot) {
                crate::clippage::dropped(self.hwnd(), formats);
                effect = DROPEFFECT_COPY;
            } else {
                crate::clippage::drag_hover(self.hwnd(), false);
            }
        }
        unsafe { *pdweffect = effect };
        Ok(())
    }
}

/// Let the notch window receive drops.
pub fn register_drop_target(hwnd: HWND) {
    let target: IDropTarget = DropTarget { hwnd: hwnd.0 as isize, accepting: Cell::new(false) }.into();
    if let Err(e) = unsafe { RegisterDragDrop(hwnd, &target) } {
        // Nothing to show it on: the notch simply cannot take drops (copying and dragging out still work).
        eprintln!("notch: drag and drop in is unavailable: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_object_serves_what_it_holds() {
        let data: IDataObject = DataObject { formats: vec![(CF_UNICODETEXT, vec![104, 0, 105, 0, 0, 0]), (CF_DIB, vec![7; 3])] }.into();
        unsafe {
            assert_eq!(data.QueryGetData(&query(CF_UNICODETEXT)), S_OK);
            assert_ne!(data.QueryGetData(&query(CF_HDROP)), S_OK);
            assert!(accepts(&data));
            let mut medium = data.GetData(&query(CF_DIB)).unwrap();
            let h = medium.u.hGlobal;
            let bytes = std::slice::from_raw_parts(GlobalLock(h).cast::<u8>(), GlobalSize(h)).to_vec();
            let _ = GlobalUnlock(h);
            ReleaseStgMedium(&mut medium);
            assert_eq!(bytes, [7, 7, 7]);
            assert!(data.GetData(&query(CF_HDROP)).is_err());
        }
    }

    // Overwrites the user's clipboard, so it only runs on request: cargo test -- --ignored set_clipboard
    #[test]
    #[ignore]
    fn set_clipboard_reaches_the_system_clipboard() {
        unsafe { OleInitialize(None).unwrap() };
        let text: Vec<u8> = "notch clipboard test".encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        assert!(set_clipboard(vec![(CF_UNICODETEXT, text.clone())]));
        let back = clipboard_snapshot().expect("clipboard readable");
        assert_eq!(back, [(CF_UNICODETEXT, text)]);
    }

    #[test]
    fn snapshot_reads_back_what_a_data_object_holds() {
        let text = vec![104, 0, 105, 0, 0, 0];
        let data: IDataObject = DataObject { formats: vec![(CF_UNICODETEXT, text.clone()), (CF_DIB, vec![1; 12])] }.into();
        let mut got = snapshot(&data).unwrap();
        got.sort();
        assert_eq!(got, [(CF_DIB, vec![1; 12]), (CF_UNICODETEXT, text)]);
    }
}
