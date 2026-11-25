/* Zig's x86_64 Windows code emits `call __chkstk; sub %rax, %rsp`: the helper
 * must probe the new stack pages but must NOT move RSP itself, and must leave
 * RAX (the requested size) and the argument registers RCX/RDX/R8 untouched.
 * MinGW libgcc's `___chkstk` adjusts RSP and `___chkstk_ms` clobbers RCX, so
 * neither can be used here. Probe from the bottom of the frame upward. */
    .globl  __chkstk
    .p2align 4, 0x90
__chkstk:
    mov     %rsp, %r10
    sub     %rax, %r10
    and     $-16, %r10
.Lpain_chkstk_probe:
    orl     $0, (%r10)
    add     $0x1000, %r10
    cmp     %rsp, %r10
    jb      .Lpain_chkstk_probe
    ret
