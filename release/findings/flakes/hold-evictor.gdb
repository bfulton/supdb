set pagination off
set non-stop on
starti
break '_RNvMs0_NtNtCslgyNmJ89Ydv_5supdb2db5pagerNtB5_5Pager3run'
commands
  silent
  printf "HELD EVICTOR\n"
  shell sleep 0.5
  continue
end
continue -a &
