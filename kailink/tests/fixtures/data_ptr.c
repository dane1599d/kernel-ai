I32 cells[4] = {1, 2, 3, 4};
I32 *third = &cells[2];
I64 __kai_run_p(void) { return *third + cells[3]; }
