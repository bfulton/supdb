 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_>:
 push %rbp
 push %rbx
 sub $0x18,%rsp
 cmpl $0xffffffff,0x30(%rdi)
 je 200109 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xf9>
 mov 0x120(%rdi),%rax
 mov 0x130(%rdi),%rcx
 cmp $0xffffffffffffffff,%rax
 je 200040 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0x30>
 mov 0x128(%rdi),%rdx
 mov %rdx,%r9
 jmp 200069 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0x59>
 mov 0x128(%rdi),%rdx
 mov %rdx,%r8
 add %rcx,%r8
 jb 2000e2 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xd2>
 cmp 0x20(%rdi),%r8
 ja 2000e2 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xd2>
 mov 0x18(%rdi),%r9
 test %r9,%r9
 je 2000e2 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xd2>
 add %rdx,%r9
 mov 0x90(%rdi),%r8
 mov 0x98(%rdi),%r10
 cmp %r8,%r10
 setb %r11b
 cmp %rcx,%r10
 seta %r10b
 or %r11b,%r10b
 jne 200109 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xf9>
 mov 0x70(%rdi),%r10
 mov 0x78(%rdi),%rbx
 mov %rbx,%r11
 sub %r10,%r11
 setae %bpl
 cmp %rcx,%rbx
 setbe %cl
 test %cl,%bpl
 je 200109 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xf9>
 cmp 0xb0(%rdi),%rsi
 jae 200109 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xf9>
 lea 0x3(,%rsi,4),%rcx
 cmp %r11,%rcx
 jae 200109 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xf9>
 add %r10,%r9
 shl $0x2,%rsi
 mov (%r9,%rsi,1),%ecx
 cmp $0xffffffffffffffff,%rax
 je 2000d5 <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xc5>
 mov 0x138(%rdi),%rdx
 add %rcx,%r8
 add %rdx,%r8
 mov $0x1,%eax
 jmp 20010b <_RNvMs1_NtCsckwRMzVZNXF_5supdb4blobINtB5_4BlobNtNtB7_5bytes9MmapBytesE13record_offsetB7_+0xfb>
 mov (%rdi),%rax
 mov 0x28(%rdi),%rsi
 mov %rax,%rdi
 call bea00 <_RINvNtCsckwRMzVZNXF_5supdb4blob8not_lentNtNtB4_5bytes9MmapBytesEB4_>
 lea 0x10(%rsp),%rdi
 mov %rax,0x10(%rsp)
 movq $0x0,0x8(%rsp)
 call bea90 <_RINvNtCscI6d9CVNmLh_4core3ptr9drop_glueNtNtNtCsgczF5crJ4sT_3std2io5error5ErrorECsckwRMzVZNXF_5supdb>
 xor %eax,%eax
 mov %r8,%rdx
 add $0x18,%rsp
 pop %rbx
 pop %rbp
 ret

 of section .init:

 of section .fini:

 of section .plt:
