#[allow(non_snake_case)]
pub unsafe fn MojoInitialize(options: *const MojoInitializeOptions) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .Initialize
        .expect(concat!("missing Mojo thunk: ", stringify!(Initialize)));
    unsafe { function(options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoGetTimeTicksNow() -> MojoTimeTicks {
    let table = crate::system_thunks();
    let function = table
        .GetTimeTicksNow
        .expect(concat!("missing Mojo thunk: ", stringify!(GetTimeTicksNow)));
    unsafe { function() }
}
#[allow(non_snake_case)]
pub unsafe fn MojoClose(handle: MojoHandle) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .Close
        .expect(concat!("missing Mojo thunk: ", stringify!(Close)));
    unsafe { function(handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoQueryHandleSignalsState(
    handle: MojoHandle,
    signals_state: *mut MojoHandleSignalsState,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .QueryHandleSignalsState
        .expect(concat!("missing Mojo thunk: ", stringify!(QueryHandleSignalsState)));
    unsafe { function(handle, signals_state) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoCreateMessagePipe(
    options: *const MojoCreateMessagePipeOptions,
    message_pipe_handle0: *mut MojoHandle,
    message_pipe_handle1: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .CreateMessagePipe
        .expect(concat!("missing Mojo thunk: ", stringify!(CreateMessagePipe)));
    unsafe { function(options, message_pipe_handle0, message_pipe_handle1) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoWriteMessage(
    message_pipe_handle: MojoHandle,
    message_handle: MojoMessageHandle,
    options: *const MojoWriteMessageOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .WriteMessage
        .expect(concat!("missing Mojo thunk: ", stringify!(WriteMessage)));
    unsafe { function(message_pipe_handle, message_handle, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoReadMessage(
    message_pipe_handle: MojoHandle,
    options: *const MojoReadMessageOptions,
    message_handle: *mut MojoMessageHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .ReadMessage
        .expect(concat!("missing Mojo thunk: ", stringify!(ReadMessage)));
    unsafe { function(message_pipe_handle, options, message_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoFuseMessagePipes(
    handle0: MojoHandle,
    handle1: MojoHandle,
    options: *const MojoFuseMessagePipesOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .FuseMessagePipes
        .expect(concat!("missing Mojo thunk: ", stringify!(FuseMessagePipes)));
    unsafe { function(handle0, handle1, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoCreateMessage(
    options: *const MojoCreateMessageOptions,
    message: *mut MojoMessageHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .CreateMessage
        .expect(concat!("missing Mojo thunk: ", stringify!(CreateMessage)));
    unsafe { function(options, message) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoDestroyMessage(message: MojoMessageHandle) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .DestroyMessage
        .expect(concat!("missing Mojo thunk: ", stringify!(DestroyMessage)));
    unsafe { function(message) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoSerializeMessage(
    message: MojoMessageHandle,
    options: *const MojoSerializeMessageOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .SerializeMessage
        .expect(concat!("missing Mojo thunk: ", stringify!(SerializeMessage)));
    unsafe { function(message, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoAppendMessageData(
    message: MojoMessageHandle,
    additional_payload_size: u32,
    handles: *const MojoHandle,
    num_handles: u32,
    options: *const MojoAppendMessageDataOptions,
    buffer: *mut *mut ::std::os::raw::c_void,
    buffer_size: *mut u32,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .AppendMessageData
        .expect(concat!("missing Mojo thunk: ", stringify!(AppendMessageData)));
    unsafe {
        function(
            message,
            additional_payload_size,
            handles,
            num_handles,
            options,
            buffer,
            buffer_size,
        )
    }
}
#[allow(non_snake_case)]
pub unsafe fn MojoGetMessageData(
    message: MojoMessageHandle,
    options: *const MojoGetMessageDataOptions,
    buffer: *mut *mut ::std::os::raw::c_void,
    num_bytes: *mut u32,
    handles: *mut MojoHandle,
    num_handles: *mut u32,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .GetMessageData
        .expect(concat!("missing Mojo thunk: ", stringify!(GetMessageData)));
    unsafe { function(message, options, buffer, num_bytes, handles, num_handles) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoSetMessageContext(
    message: MojoMessageHandle,
    context: usize,
    serializer: MojoMessageContextSerializer,
    destructor: MojoMessageContextDestructor,
    options: *const MojoSetMessageContextOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .SetMessageContext
        .expect(concat!("missing Mojo thunk: ", stringify!(SetMessageContext)));
    unsafe { function(message, context, serializer, destructor, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoGetMessageContext(
    message: MojoMessageHandle,
    options: *const MojoGetMessageContextOptions,
    context: *mut usize,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .GetMessageContext
        .expect(concat!("missing Mojo thunk: ", stringify!(GetMessageContext)));
    unsafe { function(message, options, context) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoNotifyBadMessage(
    message: MojoMessageHandle,
    error: *const ::std::os::raw::c_char,
    error_num_bytes: u32,
    options: *const MojoNotifyBadMessageOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .NotifyBadMessage
        .expect(concat!("missing Mojo thunk: ", stringify!(NotifyBadMessage)));
    unsafe { function(message, error, error_num_bytes, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoCreateDataPipe(
    options: *const MojoCreateDataPipeOptions,
    data_pipe_producer_handle: *mut MojoHandle,
    data_pipe_consumer_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .CreateDataPipe
        .expect(concat!("missing Mojo thunk: ", stringify!(CreateDataPipe)));
    unsafe { function(options, data_pipe_producer_handle, data_pipe_consumer_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoWriteData(
    data_pipe_producer_handle: MojoHandle,
    elements: *const ::std::os::raw::c_void,
    num_elements: *mut u32,
    options: *const MojoWriteDataOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .WriteData
        .expect(concat!("missing Mojo thunk: ", stringify!(WriteData)));
    unsafe { function(data_pipe_producer_handle, elements, num_elements, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoBeginWriteData(
    data_pipe_producer_handle: MojoHandle,
    options: *const MojoBeginWriteDataOptions,
    buffer: *mut *mut ::std::os::raw::c_void,
    buffer_num_elements: *mut u32,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .BeginWriteData
        .expect(concat!("missing Mojo thunk: ", stringify!(BeginWriteData)));
    unsafe { function(data_pipe_producer_handle, options, buffer, buffer_num_elements) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoEndWriteData(
    data_pipe_producer_handle: MojoHandle,
    num_elements_written: u32,
    options: *const MojoEndWriteDataOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .EndWriteData
        .expect(concat!("missing Mojo thunk: ", stringify!(EndWriteData)));
    unsafe { function(data_pipe_producer_handle, num_elements_written, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoReadData(
    data_pipe_consumer_handle: MojoHandle,
    options: *const MojoReadDataOptions,
    elements: *mut ::std::os::raw::c_void,
    num_elements: *mut u32,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .ReadData
        .expect(concat!("missing Mojo thunk: ", stringify!(ReadData)));
    unsafe { function(data_pipe_consumer_handle, options, elements, num_elements) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoBeginReadData(
    data_pipe_consumer_handle: MojoHandle,
    options: *const MojoBeginReadDataOptions,
    buffer: *mut *const ::std::os::raw::c_void,
    buffer_num_elements: *mut u32,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .BeginReadData
        .expect(concat!("missing Mojo thunk: ", stringify!(BeginReadData)));
    unsafe { function(data_pipe_consumer_handle, options, buffer, buffer_num_elements) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoEndReadData(
    data_pipe_consumer_handle: MojoHandle,
    num_elements_read: u32,
    options: *const MojoEndReadDataOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .EndReadData
        .expect(concat!("missing Mojo thunk: ", stringify!(EndReadData)));
    unsafe { function(data_pipe_consumer_handle, num_elements_read, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoCreateSharedBuffer(
    num_bytes: u64,
    options: *const MojoCreateSharedBufferOptions,
    shared_buffer_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .CreateSharedBuffer
        .expect(concat!("missing Mojo thunk: ", stringify!(CreateSharedBuffer)));
    unsafe { function(num_bytes, options, shared_buffer_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoDuplicateBufferHandle(
    buffer_handle: MojoHandle,
    options: *const MojoDuplicateBufferHandleOptions,
    new_buffer_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .DuplicateBufferHandle
        .expect(concat!("missing Mojo thunk: ", stringify!(DuplicateBufferHandle)));
    unsafe { function(buffer_handle, options, new_buffer_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoMapBuffer(
    buffer_handle: MojoHandle,
    offset: u64,
    num_bytes: u64,
    options: *const MojoMapBufferOptions,
    buffer: *mut *mut ::std::os::raw::c_void,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .MapBuffer
        .expect(concat!("missing Mojo thunk: ", stringify!(MapBuffer)));
    unsafe { function(buffer_handle, offset, num_bytes, options, buffer) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoUnmapBuffer(buffer: *mut ::std::os::raw::c_void) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .UnmapBuffer
        .expect(concat!("missing Mojo thunk: ", stringify!(UnmapBuffer)));
    unsafe { function(buffer) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoGetBufferInfo(
    buffer_handle: MojoHandle,
    options: *const MojoGetBufferInfoOptions,
    info: *mut MojoSharedBufferInfo,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .GetBufferInfo
        .expect(concat!("missing Mojo thunk: ", stringify!(GetBufferInfo)));
    unsafe { function(buffer_handle, options, info) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoCreateTrap(
    handler: MojoTrapEventHandler,
    options: *const MojoCreateTrapOptions,
    trap_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .CreateTrap
        .expect(concat!("missing Mojo thunk: ", stringify!(CreateTrap)));
    unsafe { function(handler, options, trap_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoAddTrigger(
    trap_handle: MojoHandle,
    handle: MojoHandle,
    signals: MojoHandleSignals,
    condition: MojoTriggerCondition,
    context: usize,
    options: *const MojoAddTriggerOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .AddTrigger
        .expect(concat!("missing Mojo thunk: ", stringify!(AddTrigger)));
    unsafe { function(trap_handle, handle, signals, condition, context, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoRemoveTrigger(
    trap_handle: MojoHandle,
    context: usize,
    options: *const MojoRemoveTriggerOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .RemoveTrigger
        .expect(concat!("missing Mojo thunk: ", stringify!(RemoveTrigger)));
    unsafe { function(trap_handle, context, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoArmTrap(
    trap_handle: MojoHandle,
    options: *const MojoArmTrapOptions,
    num_blocking_events: *mut u32,
    blocking_events: *mut MojoTrapEvent,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .ArmTrap
        .expect(concat!("missing Mojo thunk: ", stringify!(ArmTrap)));
    unsafe { function(trap_handle, options, num_blocking_events, blocking_events) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoWrapPlatformHandle(
    platform_handle: *const MojoPlatformHandle,
    options: *const MojoWrapPlatformHandleOptions,
    mojo_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .WrapPlatformHandle
        .expect(concat!("missing Mojo thunk: ", stringify!(WrapPlatformHandle)));
    unsafe { function(platform_handle, options, mojo_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoUnwrapPlatformHandle(
    mojo_handle: MojoHandle,
    options: *const MojoUnwrapPlatformHandleOptions,
    platform_handle: *mut MojoPlatformHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .UnwrapPlatformHandle
        .expect(concat!("missing Mojo thunk: ", stringify!(UnwrapPlatformHandle)));
    unsafe { function(mojo_handle, options, platform_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoWrapPlatformSharedMemoryRegion(
    platform_handles: *const MojoPlatformHandle,
    num_platform_handles: u32,
    num_bytes: u64,
    guid: *const MojoSharedBufferGuid,
    access_mode: MojoPlatformSharedMemoryRegionAccessMode,
    options: *const MojoWrapPlatformSharedMemoryRegionOptions,
    mojo_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .WrapPlatformSharedMemoryRegion
        .expect(
            concat!("missing Mojo thunk: ", stringify!(WrapPlatformSharedMemoryRegion)),
        );
    unsafe {
        function(
            platform_handles,
            num_platform_handles,
            num_bytes,
            guid,
            access_mode,
            options,
            mojo_handle,
        )
    }
}
#[allow(non_snake_case)]
pub unsafe fn MojoUnwrapPlatformSharedMemoryRegion(
    mojo_handle: MojoHandle,
    options: *const MojoUnwrapPlatformSharedMemoryRegionOptions,
    platform_handles: *mut MojoPlatformHandle,
    num_platform_handles: *mut u32,
    num_bytes: *mut u64,
    guid: *mut MojoSharedBufferGuid,
    access_mode: *mut MojoPlatformSharedMemoryRegionAccessMode,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .UnwrapPlatformSharedMemoryRegion
        .expect(
            concat!("missing Mojo thunk: ", stringify!(UnwrapPlatformSharedMemoryRegion)),
        );
    unsafe {
        function(
            mojo_handle,
            options,
            platform_handles,
            num_platform_handles,
            num_bytes,
            guid,
            access_mode,
        )
    }
}
#[allow(non_snake_case)]
pub unsafe fn MojoCreateInvitation(
    options: *const MojoCreateInvitationOptions,
    invitation_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .CreateInvitation
        .expect(concat!("missing Mojo thunk: ", stringify!(CreateInvitation)));
    unsafe { function(options, invitation_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoAttachMessagePipeToInvitation(
    invitation_handle: MojoHandle,
    name: *const ::std::os::raw::c_void,
    name_num_bytes: u32,
    options: *const MojoAttachMessagePipeToInvitationOptions,
    message_pipe_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .AttachMessagePipeToInvitation
        .expect(
            concat!("missing Mojo thunk: ", stringify!(AttachMessagePipeToInvitation)),
        );
    unsafe {
        function(invitation_handle, name, name_num_bytes, options, message_pipe_handle)
    }
}
#[allow(non_snake_case)]
pub unsafe fn MojoExtractMessagePipeFromInvitation(
    invitation_handle: MojoHandle,
    name: *const ::std::os::raw::c_void,
    name_num_bytes: u32,
    options: *const MojoExtractMessagePipeFromInvitationOptions,
    message_pipe_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .ExtractMessagePipeFromInvitation
        .expect(
            concat!("missing Mojo thunk: ", stringify!(ExtractMessagePipeFromInvitation)),
        );
    unsafe {
        function(invitation_handle, name, name_num_bytes, options, message_pipe_handle)
    }
}
#[allow(non_snake_case)]
pub unsafe fn MojoSendInvitation(
    invitation_handle: MojoHandle,
    process_handle: *const MojoPlatformProcessHandle,
    transport_endpoint: *const MojoInvitationTransportEndpoint,
    error_handler: MojoProcessErrorHandler,
    error_handler_context: usize,
    options: *const MojoSendInvitationOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .SendInvitation
        .expect(concat!("missing Mojo thunk: ", stringify!(SendInvitation)));
    unsafe {
        function(
            invitation_handle,
            process_handle,
            transport_endpoint,
            error_handler,
            error_handler_context,
            options,
        )
    }
}
#[allow(non_snake_case)]
pub unsafe fn MojoAcceptInvitation(
    transport_endpoint: *const MojoInvitationTransportEndpoint,
    options: *const MojoAcceptInvitationOptions,
    invitation_handle: *mut MojoHandle,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .AcceptInvitation
        .expect(concat!("missing Mojo thunk: ", stringify!(AcceptInvitation)));
    unsafe { function(transport_endpoint, options, invitation_handle) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoSetQuota(
    handle: MojoHandle,
    type_: MojoQuotaType,
    limit: u64,
    options: *const MojoSetQuotaOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .SetQuota
        .expect(concat!("missing Mojo thunk: ", stringify!(SetQuota)));
    unsafe { function(handle, type_, limit, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoQueryQuota(
    handle: MojoHandle,
    type_: MojoQuotaType,
    options: *const MojoQueryQuotaOptions,
    limit: *mut u64,
    usage: *mut u64,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .QueryQuota
        .expect(concat!("missing Mojo thunk: ", stringify!(QueryQuota)));
    unsafe { function(handle, type_, options, limit, usage) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoShutdown(options: *const MojoShutdownOptions) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .Shutdown
        .expect(concat!("missing Mojo thunk: ", stringify!(Shutdown)));
    unsafe { function(options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoSetDefaultProcessErrorHandler(
    handler: MojoDefaultProcessErrorHandler,
    options: *const MojoSetDefaultProcessErrorHandlerOptions,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .SetDefaultProcessErrorHandler
        .expect(
            concat!("missing Mojo thunk: ", stringify!(SetDefaultProcessErrorHandler)),
        );
    unsafe { function(handler, options) }
}
#[allow(non_snake_case)]
pub unsafe fn MojoReserveMessageCapacity(
    message: MojoMessageHandle,
    payload_buffer_size: u32,
    buffer_size: *mut u32,
) -> MojoResult {
    let table = crate::system_thunks();
    let function = table
        .ReserveMessageCapacity
        .expect(concat!("missing Mojo thunk: ", stringify!(ReserveMessageCapacity)));
    unsafe { function(message, payload_buffer_size, buffer_size) }
}
