set pagination off
set non-stop on
starti
break *('_RNvMs23_NtCsdLdSKQ2MNhB_5supdb2dbNtB6_2Db13maint_request'+316)
commands
  silent
  printf "HIT\n"
  shell sleep 0.3
  continue
end
continue -a &
