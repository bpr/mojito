# Runs on both compilers (re-probed 2026-09-08 against the pinned build):
# `unsafe_alloc` imports from `std.memory.alloc`, as upstream (the `std.memory`
# package does not export it), and both print `20 1 True`.
from std.memory.alloc import unsafe_alloc

def main():
    var base = unsafe_alloc[Int](4, alignment=16)
    base[0] = 10
    base[1] = 20
    var next = base + 1
    print(next[0], next - base, next == base + 1)
    base.free()
