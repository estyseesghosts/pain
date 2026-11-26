/* Windows stack probes.
 *
 * Zig's x86_64 Windows code emits `call __chkstk; sub %rax, %rsp`: the helper
 * must probe the new stack pages but must NOT move RSP itself, and must leave
 * RAX (the requested size) and the argument registers RCX/RDX/R8 untouched.
 *
 * Rust and GCC call `___chkstk_ms` and subtract RAX themselves after the
 * helper returns. MinGW's implementation probes from the current stack
 * downward; probing from the bottom of the new frame upward skips the guard
 * page and faults on a large frame in a thread with a small committed stack.
 * Keep the two helper entry points separate so each ABI gets the probe order
 * it requires. */

    .globl  __chkstk
    .p2align 4, 0x90
__chkstk:
    mov     %rsp, %r10
    sub     $0x1000, %r10
    mov     %rsp, %r11
    sub     %rax, %r11
.Lpain_chkstk_probe:
    orl     $0, (%r10)
    cmp     %r11, %r10
    jbe     .Lpain_chkstk_done
    sub     $0x1000, %r10
    jmp     .Lpain_chkstk_probe
.Lpain_chkstk_done:
    ret

    .globl  ___chkstk_ms
    .p2align 4, 0x90
___chkstk_ms:
    push    %rcx
    push    %rax
    cmp     $0x1000, %rax
    lea     0x18(%rsp), %rcx
    jb      .Lpain_chkstk_ms_tail
.Lpain_chkstk_ms_loop:
    sub     $0x1000, %rcx
    orq     $0, (%rcx)
    sub     $0x1000, %rax
    cmp     $0x1000, %rax
    ja      .Lpain_chkstk_ms_loop
.Lpain_chkstk_ms_tail:
    sub     %rax, %rcx
    orq     $0, (%rcx)
    pop     %rax
    pop     %rcx
    ret
