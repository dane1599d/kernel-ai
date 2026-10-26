I64 sum(I64 n) {
  I64 s = 0;
  for (I64 i = 0; i < n; ++i) s += i ^ (i >> 3);
  return s;
}
I64 __kai_run_sum(void) { return sum(10000000); }
