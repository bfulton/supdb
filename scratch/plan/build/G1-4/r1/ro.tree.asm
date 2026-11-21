 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_>:
 push %rbp
 push %r14
 push %rbx
 sub $0x10,%rsp
 cmpl $0xffffffff,0x30(%rdi)
 je 200abb <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0x11b>
 mov 0x120(%rdi),%rax
 mov 0x130(%rdi),%rcx
 cmp $0xffffffffffffffff,%rax
 je 2009d2 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0x32>
 mov 0x128(%rdi),%rdx
 mov %rdx,%r9
 jmp 2009ff <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0x5f>
 mov 0x128(%rdi),%rdx
 mov %rdx,%r8
 add %rcx,%r8
 jb 200a86 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xe6>
 cmp 0x20(%rdi),%r8
 ja 200a86 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xe6>
 mov 0x18(%rdi),%r9
 test %r9,%r9
 je 200a86 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xe6>
 add %rdx,%r9
 mov 0x90(%rdi),%r8
 mov 0x98(%rdi),%r10
 cmp %r8,%r10
 setb %r11b
 cmp %rcx,%r10
 seta %r10b
 or %r11b,%r10b
 jne 200a78 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xd8>
 mov 0x70(%rdi),%r10
 mov 0x78(%rdi),%rbx
 mov %rbx,%r11
 sub %r10,%r11
 setb %bpl
 cmp %rcx,%rbx
 seta %cl
 or %bpl,%cl
 jne 200a78 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xd8>
 cmp 0xb0(%rdi),%rsi
 jae 200a78 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xd8>
 lea 0x3(,%rsi,4),%rcx
 cmp %r11,%rcx
 jae 200a78 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xd8>
 add %r10,%r9
 shl $0x2,%rsi
 mov (%r9,%rsi,1),%ecx
 cmp $0xffffffffffffffff,%rax
 je 200a6b <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xcb>
 mov 0x138(%rdi),%rdx
 add %rcx,%r8
 add %rdx,%r8
 mov $0x1,%eax
 jmp 200a7a <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xda>
 xor %eax,%eax
 mov %r8,%rdx
 add $0x10,%rsp
 pop %rbx
 pop %r14
 pop %rbp
 ret
 mov (%rdi),%rax
 mov 0x28(%rdi),%r8
 mov %rdi,%rbx
 mov %rax,%rdi
 mov %rsi,%r14
 mov %r8,%rsi
 call bebf0 <_RINvNtCsckwRMzVZNXF_5supdb4blob8not_lentNtNtB4_5bytes9MmapBytesEB4_>
 lea 0x8(%rsp),%rdi
 mov %rax,0x8(%rsp)
 movq $0x0,(%rsp)
 call bec80 <_RINvNtCscI6d9CVNmLh_4core3ptr9drop_glueNtNtNtCsgczF5crJ4sT_3std2io5error5ErrorECsckwRMzVZNXF_5supdb>
 mov %r14,%rsi
 mov %rbx,%rdi
 add $0x10,%rsp
 pop %rbx
 pop %r14
 pop %rbp
 jmp 200ad0 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE22record_offset_by_pieceB7_>

 of section .init:

 of section .fini:

 of section .plt:
