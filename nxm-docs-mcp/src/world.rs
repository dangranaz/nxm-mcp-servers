//! Minimal `typst::World` implementation for in-memory compilation.
//!
//! Provides the Typst compiler with the generated Typst source and system fonts.

use typst::foundations::Bytes;
use typst::syntax::{FileId, RootedPath, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};

use typst_kit::fonts::FontStore;

/// The virtual path for our single in-memory source file.
const MAIN_VPATH: &str = "/main.typ";

/// A `World` that serves a single in-memory Typst source file and system fonts.
pub struct NxmWorld {
    source: Source,
    library: LazyHash<Library>,
    font_store: FontStore,
}

impl NxmWorld {
    /// Create a new world from a Typst markup string.
    pub fn new(typst_src: &str) -> Self {
        let vpath = VirtualPath::new(MAIN_VPATH).expect("invalid virtual path");
        let rooted = RootedPath::new(typst::syntax::VirtualRoot::Project, vpath);
        let id = FileId::new(rooted);
        let source = Source::new(id, typst_src.to_string());

        let library = LazyHash::new(Library::builder().build());

        let mut font_store = FontStore::new();
        // Register system fonts as a fallback
        font_store.extend(typst_kit::fonts::system());

        Self {
            source,
            library,
            font_store,
        }
    }
}

impl World for NxmWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.font_store.book()
    }

    fn main(&self) -> FileId {
        self.source.id()
    }

    fn source(&self, id: FileId) -> Result<Source, typst::diag::FileError> {
        if id == self.source.id() {
            Ok(self.source.clone())
        } else {
            Err(typst::diag::FileError::NotFound(
                id.vpath().get_without_slash().into(),
            ))
        }
    }

    fn file(&self, id: FileId) -> Result<Bytes, typst::diag::FileError> {
        if id == self.source.id() {
            Ok(Bytes::from_string(self.source.text().to_string()))
        } else {
            Err(typst::diag::FileError::NotFound(
                id.vpath().get_without_slash().into(),
            ))
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.font_store.font(index)
    }

    fn today(&self, _offset: Option<typst::foundations::Duration>) -> Option<typst::foundations::Datetime> {
        None
    }
}
