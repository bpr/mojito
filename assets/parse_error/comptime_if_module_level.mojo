# expect: 'comptime if' must be contained in a function
# Upstream requires a `comptime if` inside a function; at module level it
# rejects with this sentence. A fixture that wants the same fold outside one
# writes a second `comptime` alias instead (see
# `assets/ok/type_predicate_comptime_if.mojo`).
comptime N = 4

comptime if N > 2:
    comptime M = 1

def main():
    print(N)
