// Copyright 2026 The Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module defines (mostly) safe Rust wrappers around the Mojo message API.
//! Note that this is distinct from the message _pipe_ API in the `message_pipe`
//! module.
//!
//! Not all C API functions are included yet. More can be added as-needed by
//! following the example of existing wrappers.

use crate::internal_options::declare_mojo_options;

use crate::handles::*;
use crate::result::*;

bitflags::bitflags! {
    /// The possible flags that can be passed to MojoCreateMessage
    #[derive(Clone, Copy, Default)]
    pub struct CreateMessageFlags: u32 {
        /// Do not enforce size restrictions on this message.
        const UNLIMITED_SIZE = 1 << 0;
    }
}
declare_mojo_options!(MojoCreateMessageOptions, flags: raw_ffi::MojoCreateMessageFlags);

/// Create a new message object which can be sent over a message pipe.
///
/// In its initial state the message object cannot be successfully written to a
/// message pipe, but must first have either an opaque context or
/// some serialized data attached (see `MojoSetMessageContext` and
/// `MojoAppendMessageData`).
///
/// Each message object contains a *payload* with two components: a chunk of raw
/// bytes and a list of `UntypedHandle`s. Data can be written to the payload
/// using `MojoAppendMessageData` and read using `MojoGetMessageData`.
/// Appending/reading handles transfers ownership of those handlers to/from the
/// message.
///
/// # Flags:
/// - UNLIMITED_SIZE: If passed, the message's payload may grow to an arbitrary
///   size. Otherwise, Mojo will assert-fail if its payload grows past a
///   globally configured threshold.
pub fn MojoCreateMessage(flags: CreateMessageFlags) -> MessageHandle {
    let options = MojoCreateMessageOptions::new(flags.bits());
    let mut message_handle: raw_ffi::MojoMessageHandle = 0;

    // SAFETY: Both pointers are to stack data
    // Surprisingly, according to its comments, this function will not fail
    // unless the message pointer is null, which it isn't.
    unsafe {
        raw_ffi::MojoCreateMessage(options.as_ptr(), std::ptr::from_mut(&mut message_handle))
    };

    // SAFETY: we just got this handle from Mojo
    unsafe { MessageHandle::wrap_raw_value(message_handle) }
}

bitflags::bitflags! {
    /// The possible flags that can be passed to MojoAppendMessageData
    #[derive(Clone, Copy, Default)]
    pub struct AppendMessageDataFlags: u32 {
        /// Indicates that the message is now complete and its size will no longer increase.
        const COMMIT_SIZE = 1 << 0;
    }
}
declare_mojo_options!(MojoAppendMessageDataOptions, flags: raw_ffi::MojoAppendMessageDataFlags);

fn checked_parts_size(lengths: impl IntoIterator<Item = usize>) -> MojoResult<u32> {
    let mut size = 0u32;
    for length in lengths {
        let length = u32::try_from(length).map_err(|_| MojoError::ResourceExhausted)?;
        size = size
            .checked_add(length)
            .ok_or(MojoError::ResourceExhausted)?;
    }
    // Pointer arithmetic within the destination allocation must also fit in
    // isize, including on 32-bit Rust hosts.
    if u64::from(size) > isize::MAX as u64 {
        return Err(MojoError::ResourceExhausted);
    }
    Ok(size)
}

/// Create a fresh, fully committed message from ordered byte slices.
///
/// This convenience constructor reserves the complete payload in one native
/// append and copies each part directly into that buffer. It never concatenates
/// the parts in an intermediate Rust allocation or appends to an existing
/// message. Empty parts and an empty payload are permitted.
///
/// Ownership of `handles` transfers to the new message only if the native
/// append succeeds. Every ordinary error returns the original handles unchanged.
/// Payload/count overflow returns `ResourceExhausted` before calling Mojo.
///
/// # Panics
/// Panics if the trusted native runtime violates its successful-append buffer
/// contract. At that point the native message owns the handles, and unwinding
/// destroys that message; those handles must not be returned or closed twice.
pub fn MojoCreateMessageWithDataParts(
    parts: &[&[u8]],
    handles: Vec<UntypedHandle>,
) -> Result<MessageHandle, (MojoError, Vec<UntypedHandle>)> {
    let payload_size = match checked_parts_size(parts.iter().map(|part| part.len())) {
        Ok(size) => size,
        Err(error) => return Err((error, handles)),
    };
    let num_handles = match u32::try_from(handles.len()) {
        Ok(count) => count,
        Err(_) => return Err((MojoError::ResourceExhausted, handles)),
    };
    let message = MojoCreateMessage(CreateMessageFlags::empty());
    let options = MojoAppendMessageDataOptions::new(AppendMessageDataFlags::COMMIT_SIZE.bits());
    let mut buffer: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut capacity = 0u32;
    // SAFETY: this message is fresh and uniquely owned. Input handles are live
    // and uniquely owned by this Vec; their pointer is null exactly when empty.
    // All output pointers refer to initialized writable stack variables.
    let result = MojoError::result_from_code(unsafe {
        raw_ffi::MojoAppendMessageData(
            message.handle_value.into(),
            payload_size,
            UntypedHandle::slice_as_ptr(&handles),
            num_handles,
            options.as_ptr(),
            &mut buffer,
            &mut capacity,
        )
    });
    if let Err(error) = result {
        // The native API consumed no handles. The empty message is destroyed
        // here, while the caller receives every original handle back.
        return Err((error, handles));
    }
    // Successful append transfers ALL handles, even if a subsequent buffer
    // contract assertion fails. The native message then performs their cleanup.
    for handle in handles {
        std::mem::forget(handle);
    }
    assert!(
        capacity >= payload_size,
        "successful MojoAppendMessageData returned less than the requested capacity"
    );
    if payload_size != 0 {
        assert!(
            !buffer.is_null(),
            "successful MojoAppendMessageData returned a null payload buffer"
        );
        let mut offset = 0usize;
        for part in parts {
            if !part.is_empty() {
                // SAFETY: the checked sum is the message's exact logical size,
                // which fits the native capacity and isize. Fresh native storage
                // cannot alias these live input slices. copy_nonoverlapping
                // initializes the destination without forming a Rust slice over
                // the native buffer's previously uninitialized bytes. COMMIT_SIZE
                // leaves it writable until MojoWriteMessage/MojoDestroyMessage.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        part.as_ptr(),
                        buffer.cast::<u8>().add(offset),
                        part.len(),
                    );
                }
            }
            offset += part.len();
        }
    }
    Ok(message)
}

/// Append data to the message's payload. Returns the number of bytes written.
///
/// This will append the contents of `bytes` and `handles` to the message's
/// internal payload buffer. Ownership of the handles is transferred to the
/// message object if this operation succeeds; otherwise, the handles are
/// returned to the caller unchanged (either all handles are appended or none).
///
/// This function will panic if you try to append more than 2^32 handles at
/// once. If you absolutely must do so, append them in chunks.
///
/// # Flags:
/// - COMMIT_SIZE: Indicate that this is the final write operation for the
///   message. This is one way of marking the message as ready to be sent.
///
/// # Possible Error Codes:
/// - `ResourceExhausted`: if the additional payload size exceeds some
///   implementation- or embedder-defined maximum.
/// - `FailedPrecondition` if the message already has a context attached
pub fn MojoAppendMessageData(
    message: &mut MessageHandle,
    flags: AppendMessageDataFlags,
    bytes: &[u8],
    handles: Vec<UntypedHandle>,
) -> Result<usize, (MojoError, Vec<UntypedHandle>)> {
    let options = MojoAppendMessageDataOptions::new(flags.bits());
    let raw_handles_ptr: *const raw_ffi::MojoHandle = UntypedHandle::slice_as_ptr(&handles);
    let mut buffer_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut buffer_size: u32 = 0;

    // SAFETY: All pointers are to stack data, except possibly
    // `raw_handles_ptr`. `slice_as_ptr` guarantees that which is null iff
    // `handles.len()` is 0; which is permitted by the function. We're
    // passing `handles.len()` as the number of handles, so we won't read
    // past the end of `handles`.
    let result = MojoError::result_from_code(unsafe {
        raw_ffi::MojoAppendMessageData(
            message.handle_value.into(),
            bytes.len().try_into().unwrap_or(u32::MAX), // Payload size
            raw_handles_ptr,
            handles.len().try_into().unwrap(), // Number of handles
            options.as_ptr(),
            std::ptr::from_mut(&mut buffer_ptr),
            std::ptr::from_mut(&mut buffer_size),
        )
    });

    match result {
        Ok(()) => {
            // If `MojoAppendMessageData` succeeds, ownership of all handles is
            // transferred to the C object, so prevent Rust from double-free-ing
            // them.
            for handle in handles.into_iter() {
                std::mem::forget(handle);
            }
        }
        Err(err) => {
            return Err((err, handles));
        }
    };

    // `MojoAppendMessageData` copies the handles for us, but returns a pointer
    // to its buffer that we need to write to ourselves.
    if !bytes.is_empty() {
        // Will not panic if usize has at least 32 bits, which is true for
        // Chromium targets
        let buffer_size: usize = buffer_size.try_into().unwrap();
        // This may be unnecessary, but `MojoAppendMessageData` doesn't
        // explicitly promise that the returned capacity is at most the
        // requested amount.
        let bytes_to_write = std::cmp::min(bytes.len(), buffer_size);

        // SAFETY: The API call guarantees that the buffer pointer is valid
        // and has at least `buffer_size` capacity, and `bytes_to_write` is
        // at most `buffer_size`.
        let buffer_slice: &mut [u8] =
            unsafe { std::slice::from_raw_parts_mut(buffer_ptr.cast(), bytes_to_write) };

        buffer_slice.copy_from_slice(&bytes[..bytes_to_write]);
        return Ok(bytes_to_write);
    };

    Ok(0)
}

bitflags::bitflags! {
    /// The possible flags that can be passed to MojoGetMessageData
    /// This struct is private because it's passed implicitly based on whether
    /// the `handles` argument to `MojoGetMessageData` is `Some` or `None`.
    #[derive(Clone, Copy, Default)]
    struct GetMessageDataFlags: u32 {
        /// Only retrieve the "raw bytes" parts of the payload
        const IGNORE_HANDLES = 1 << 0;
    }
}
declare_mojo_options!(MojoGetMessageDataOptions, flags: raw_ffi::MojoGetMessageDataFlags);

/// This type represents the result of calling GetMessageData; see the
/// documentation there for details.
///
/// The `BytesRef` parameter should be either &[u8] or &mut [u8].
//  Internally, it may also be a "raw" slice (pointer + length)
pub enum GetMessageDataStatus<BytesRef> {
    Success {
        bytes: BytesRef,
        num_handles_written: usize,
    },
    NotEnoughCapacity {
        num_handles_attached: u32,
    },
    Error(MojoError),
}

/// Retrieve the message's payload.
///
/// If `handles` is `Some` and this function is successful, it will return a
/// reference to the message's underlying bytes buffer, copy the attached
/// handles into `handles`, and return the number of handles written. This
/// transfers ownership of any attached handles to the caller. Because of the
/// ownership transfer, this function may be (successfully) called at most once
/// per message without the `IGNORE_HANDLES` flag.
///
/// If `handles` is `Some` and does not have enough capacity to store all
/// attached handles, the function will return `NotEnoughCapacity` containing
/// the number of handles attached to the message.
///
/// If `handles` is `None`, then no handles will be copied and this function
/// will never return `NotEnoughCapacity`.
///
/// If something else goes wrong, the function will return a `MojoError`, or
/// panic if `handles` has more than 2^32 elements.
///
/// # Safety Guarantees
/// - If this function returns `Success`, then the first `num_handles_written`
///   elements of `handles` have been initialized.
///
/// # Possible Error Codes:
/// - `FailedPrecondition`: If the message is not fully serialized (use
///   `MojoSerializeMessage` or `MojoAppendMessageData` to mark it as ready for
///   transfer/reading)
/// - `NotFound`: if the message's handles have already been extracted and
///   `handles` was not `None`
/// - `Aborted`: if the message is in an unrecoverable state.
// TODO(crbug.com/498966845): Add a fully-safe equivalent that allocates memory
// for the handles instead of taking a buffer as an out-parameter, and/or one
// that takes in an initialized slice to start with.
pub fn MojoGetMessageData<'a>(
    message: &'a MessageHandle,
    handles: Option<&mut [std::mem::MaybeUninit<UntypedHandle>]>,
) -> GetMessageDataStatus<&'a [u8]> {
    // Call the internal function to get a raw slice, then create a shared
    // reference to it here.
    match MojoGetMessageDataInternal(message, handles) {
        GetMessageDataStatus::Success {
            bytes: (buffer, buffer_size),
            num_handles_written,
        } => {
            GetMessageDataStatus::Success {
                bytes: if buffer_size == 0 {
                    // Core may return null for a valid empty payload. Rust
                    // slices require a non-null pointer even at length zero.
                    &[]
                } else {
                    // SAFETY: the internal function guarantees a valid,
                    // non-null buffer for this nonempty message payload.
                    unsafe { std::slice::from_raw_parts(buffer, buffer_size) }
                },
                num_handles_written,
            }
        }
        // Other return values are just passed through unchanged
        GetMessageDataStatus::NotEnoughCapacity {
            num_handles_attached,
        } => GetMessageDataStatus::NotEnoughCapacity {
            num_handles_attached,
        },
        GetMessageDataStatus::Error(err) => GetMessageDataStatus::Error(err),
    }
}

/// This function is identical to `MojoGetMessageData`, but takes and returns a
/// mutable reference.
pub fn MojoGetMessageDataMut<'a>(
    message: &'a mut MessageHandle,
    handles: Option<&mut [std::mem::MaybeUninit<UntypedHandle>]>,
) -> GetMessageDataStatus<&'a mut [u8]> {
    // Call the internal function to get a raw slice, then create a mutable
    // reference to it here.
    match MojoGetMessageDataInternal(message, handles) {
        GetMessageDataStatus::Success {
            bytes: (buffer, buffer_size),
            num_handles_written,
        } => {
            GetMessageDataStatus::Success {
                bytes: if buffer_size == 0 {
                    // As above, do not construct a Rust slice from a possibly
                    // null native pointer, even when its length is zero.
                    &mut []
                } else {
                    // SAFETY: the internal function guarantees a valid,
                    // non-null buffer, and we exclusively borrow `message`.
                    unsafe { std::slice::from_raw_parts_mut(buffer, buffer_size) }
                },
                num_handles_written,
            }
        }
        // Other return values are just passed through unchanged
        GetMessageDataStatus::NotEnoughCapacity {
            num_handles_attached,
        } => GetMessageDataStatus::NotEnoughCapacity {
            num_handles_attached,
        },
        GetMessageDataStatus::Error(err) => GetMessageDataStatus::Error(err),
    }
}

/// This function defines the core functionality used by the various
/// MojoGetMessageData functions. See the documentation for MojoGetMessageData
/// for an explanation of its behavior and requirements.
///
/// Safety considerations:
///
/// If this function returns `Success` with a nonzero size, then it is safe to
/// call `slice::from_raw_parts` on the contained pointer + size. If the caller
/// has a mutable reference to `message`, then it is also safe to call
/// `slice::from_raw_parts_mut`. A zero-size native buffer may be null and must
/// instead be represented by an ordinary empty Rust slice.
///
/// In both cases, the lifetime of the slice is the same as that of `message`.
fn MojoGetMessageDataInternal(
    message: &MessageHandle,
    handles: Option<&mut [std::mem::MaybeUninit<UntypedHandle>]>,
) -> GetMessageDataStatus<(*mut u8, usize)> {
    let flags = if handles.is_none() {
        GetMessageDataFlags::IGNORE_HANDLES
    } else {
        GetMessageDataFlags::empty()
    };
    let options = MojoGetMessageDataOptions::new(flags.bits());

    let mut buffer: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut num_bytes: u32 = 0;
    let (handles_ptr, mut num_handles) = match handles {
        // SAFETY: This cast is sound because `MaybeUninit<UntypedHandle>` has the
        // same memory layout as `UntypedHandle`, which has the same memory layout
        // as `MojoHandle`.
        Some(slice) => (
            std::ptr::from_mut(slice).cast(),
            slice.len().try_into().unwrap(),
        ),
        None => (std::ptr::null_mut(), 0),
    };

    // SAFETY: All pointers are either to stack variables, or validly null.
    // If `num_handles` is nonzero, then it contains the length of
    // `handles_ptr`, so this function will not write past the end of
    // `handles`.
    let result = MojoError::result_from_code(unsafe {
        raw_ffi::MojoGetMessageData(
            message.handle_value.into(),
            options.as_ptr(),
            std::ptr::from_mut(&mut buffer),
            std::ptr::from_mut(&mut num_bytes),
            handles_ptr,
            std::ptr::from_mut(&mut num_handles),
        )
    });

    match result {
        Ok(()) => {
            // Will not panic if usize has at least 32 bits, which is true for
            // Chromium targets
            let buffer_size: usize = num_bytes.try_into().unwrap();
            assert!(
                buffer_size == 0 || !buffer.is_null(),
                "native runtime returned a null nonempty message buffer"
            );
            // SAFETY: We must show that it is valid to call
            // `std::slice::from_raw_parts` on the `bytes` field of
            // the return value. `buffer` and `buffer_size`
            // were obtained by calling `MojoGetMessageData`, so we trust that
            // they are valid, aligned, readable, etc. Only a nonempty buffer
            // is converted from raw parts; an empty one may legally be null.
            // The buffer is readable as long as the message is alive, and the
            // type of this function ensures that the lifetime of
            // the resulting reference is the same as that of
            // `message`. If `message` is writable, then the buffer
            // is also writable.
            return GetMessageDataStatus::Success {
                bytes: (buffer.cast(), buffer_size),
                num_handles_written: num_handles.try_into().unwrap(),
            };
        }
        Err(MojoError::ResourceExhausted) => {
            // ResourceExhausted indicates that the provided handles buffer
            // wasn't large enough. In this case, `num_handles` has
            // been filled in.
            return GetMessageDataStatus::NotEnoughCapacity {
                num_handles_attached: num_handles,
            };
        }
        Err(e) => return GetMessageDataStatus::Error(e),
    }
}

/// Tell the underlying mojo system that this message was malformed, which
/// will trigger the error handler which was set up during mojo
/// initialization.
///
/// Typically this will result in the process that created the message being
/// terminated. Note that reporting a bad message does not stop the _current_
/// process from running.
// I'm not entirely sure if it's safe to call this concurrently from multiple
// threads, so I'm requiring &mut for safety.
pub fn MojoNotifyBadMessage(message: &mut MessageHandle, error_msg: &str) {
    // SAFETY: The option pointer is allowed to be be null. The string parts
    // were constructed from an &str, and will not be retained by C code
    // after the function returns.
    let ret = unsafe {
        raw_ffi::MojoNotifyBadMessage(
            message.handle_value.into(),
            error_msg.as_ptr().cast(),
            error_msg.len().try_into().unwrap(),
            std::ptr::null(), // Options pointer
        )
    };
    // This should only fail if the message handle is invalid and we guarantee
    // that it's valid as part of this type.
    debug_assert!(MojoError::result_from_code(ret).is_ok())
}

#[cfg(test)]
mod parts_tests {
    use super::*;

    #[test]
    fn parts_size_checks_native_and_pointer_limits_without_allocating() {
        assert_eq!(checked_parts_size([]), Ok(0));
        assert_eq!(checked_parts_size([0, 3, 0, 4]), Ok(7));
        let limit = (u32::MAX as usize).min(isize::MAX as usize);
        assert_eq!(checked_parts_size([limit]), Ok(limit as u32));
        assert_eq!(
            checked_parts_size([limit, 1]),
            Err(MojoError::ResourceExhausted)
        );
        assert_eq!(
            checked_parts_size([usize::MAX]),
            Err(MojoError::ResourceExhausted)
        );
    }

    #[test]
    #[ignore = "requires MOJO_NATIVE_LIB pointing to the trusted pinned SDK runtime"]
    fn fresh_parts_are_committed_and_transfer_handles_once() {
        use crate::message_pipe::{MojoCreateMessagePipe, MojoReadMessage, MojoWriteMessage};
        let library = std::path::PathBuf::from(
            std::env::var_os("MOJO_NATIVE_LIB").expect("set MOJO_NATIVE_LIB for this native test"),
        );
        // SAFETY: this opt-in test explicitly requires the authentic SDK image.
        unsafe { raw_ffi::initialize(&library, true) }.unwrap();

        fn read_local(endpoint: &UntypedHandle) -> MojoResult<MessageHandle> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match MojoReadMessage(endpoint) {
                    Err(MojoError::ShouldWait) if std::time::Instant::now() < deadline => {
                        std::thread::yield_now();
                    }
                    result => return result,
                }
            }
        }

        // A zero-length fresh message is committed even with no nonempty part.
        let empty = MojoCreateMessageWithDataParts(&[&[], &[]], vec![]).unwrap();
        match MojoGetMessageData(&empty, None) {
            GetMessageDataStatus::Success { bytes, .. } => assert!(bytes.is_empty()),
            _ => panic!("empty parts message was not committed"),
        }

        // A received empty parcel may have a null native buffer, unlike a
        // freshly reserved message. Both safe slice wrappers must accept it,
        // including when handles remain owned by that zero-byte message.
        for count in [0, 4] {
            let (attached, peers): (Vec<_>, Vec<_>) =
                (0..count).map(|_| MojoCreateMessagePipe().unwrap()).unzip();
            let (sender, receiver) = MojoCreateMessagePipe().unwrap();
            let empty = MojoCreateMessageWithDataParts(&[&[]], attached).unwrap();
            MojoWriteMessage(&sender, empty).unwrap();
            let mut received = read_local(&receiver).unwrap();
            match MojoGetMessageData(&received, None) {
                GetMessageDataStatus::Success { bytes, .. } => assert!(bytes.is_empty()),
                _ => panic!("received empty payload could not be borrowed"),
            }
            match MojoGetMessageDataMut(&mut received, None) {
                GetMessageDataStatus::Success { bytes, .. } => {
                    assert!(bytes.is_empty());
                    bytes.fill(0);
                }
                _ => panic!("received empty payload could not be mutably borrowed"),
            }
            drop(received);
            for peer in peers {
                assert!(matches!(
                    read_local(&peer),
                    Err(MojoError::FailedPrecondition)
                ));
            }
        }

        let payload: Vec<u8> = (0..1_048_576).map(|i| (i % 256) as u8).collect();
        let (attachment, attachment_peer) = MojoCreateMessagePipe().unwrap();
        let message =
            MojoCreateMessageWithDataParts(&[b"header", &[], &payload, b"tail"], vec![attachment])
                .unwrap();
        let (sender, receiver) = MojoCreateMessagePipe().unwrap();
        // No second append/commit call is made before sending.
        MojoWriteMessage(&sender, message).unwrap();
        let received = read_local(&receiver).unwrap();
        let mut handles = Vec::with_capacity(1);
        let written = match MojoGetMessageData(&received, Some(handles.spare_capacity_mut())) {
            GetMessageDataStatus::Success {
                bytes,
                num_handles_written,
            } => {
                assert_eq!(bytes.len(), 6 + payload.len() + 4);
                assert_eq!(&bytes[..6], b"header");
                assert_eq!(&bytes[6..6 + payload.len()], payload.as_slice());
                assert_eq!(&bytes[6 + payload.len()..], b"tail");
                assert_eq!(num_handles_written, 1);
                num_handles_written
            }
            _ => panic!("committed parts message could not be read"),
        };
        // SAFETY: the successful call above initialized exactly `written`
        // handles within the single reserved slot.
        unsafe { handles.set_len(written) };
        let transferred = handles.pop().unwrap();
        let ping = MojoCreateMessageWithDataParts(&[b"live"], vec![]).unwrap();
        MojoWriteMessage(&transferred, ping).unwrap();
        let pong = read_local(&attachment_peer).unwrap();
        match MojoGetMessageData(&pong, None) {
            GetMessageDataStatus::Success { bytes, .. } => assert_eq!(bytes, b"live"),
            _ => panic!("transferred attachment was not live"),
        }

        // Destruction before send closes its owned attachment, exactly once.
        let (to_drop, observer) = MojoCreateMessagePipe().unwrap();
        drop(MojoCreateMessageWithDataParts(&[b"unsent"], vec![to_drop]).unwrap());
        assert!(matches!(
            read_local(&observer),
            Err(MojoError::FailedPrecondition)
        ));
    }
}
