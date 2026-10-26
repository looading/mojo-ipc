use std::{fmt, mem::MaybeUninit};

/// C 匿名 padding 的不透明存储，不把未初始化的 padding 当成 Rust 值读取。
#[repr(transparent)]
pub struct CAnonymousPadding<Storage> {
    _storage: MaybeUninit<Storage>,
}

impl<Storage: Copy> CAnonymousPadding<Storage> {
    pub const fn new(storage: Storage) -> Self {
        Self {
            _storage: MaybeUninit::new(storage),
        }
    }

    pub const fn uninit() -> Self {
        Self {
            _storage: MaybeUninit::uninit(),
        }
    }
}

impl<Storage: Copy> Copy for CAnonymousPadding<Storage> {}

impl<Storage: Copy> Clone for CAnonymousPadding<Storage> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Storage> Default for CAnonymousPadding<Storage> {
    fn default() -> Self {
        Self {
            _storage: MaybeUninit::zeroed(),
        }
    }
}

impl<Storage> fmt::Debug for CAnonymousPadding<Storage> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // C 输出与回调只保证命名字段初始化；格式化不得读取匿名 padding。
        formatter.write_str("<anonymous C padding>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_padding_preserves_storage_layout() {
        assert_eq!(size_of::<CAnonymousPadding<[u8; 4]>>(), 4);
        assert_eq!(align_of::<CAnonymousPadding<[u8; 4]>>(), 1);
        assert_eq!(size_of::<CAnonymousPadding<u64>>(), 8);
        assert_eq!(align_of::<CAnonymousPadding<u64>>(), align_of::<u64>());
    }

    #[test]
    fn debug_and_copy_accept_uninitialized_c_padding() {
        let padding = CAnonymousPadding::<[u8; 4]>::uninit();
        let copied = padding;
        assert_eq!(format!("{padding:?}"), "<anonymous C padding>");
        assert_eq!(format!("{copied:?}"), "<anonymous C padding>");
    }
}
