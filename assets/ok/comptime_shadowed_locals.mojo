# Module comptime constants materialize shadow-aware: a local declaration,
# loop variable, or later statement in the same block that rebinds the name
# stays local instead of becoming the materialized literal (identical
# output on both compilers).
comptime n = 2 + 3
comptime i = 40

def shadowing() -> Int:
    var i = 1
    i += 1
    var total = 0
    for n in range(3):
        total += n
    return i + total

def main():
    print(n, i)
    print(shadowing())
