# Dereferencing an untracked pointer supplies the untracked origin an exact
# `ref[ImmUntrackedOrigin]` clause binds; `ref[MutUnsafeAnyOrigin]` binds any
# actual (pinned Mojo a79fbdf59f2: prints 41 then 42).
from std.memory.alloc import unsafe_alloc
def observe_untracked(ref[ImmUntrackedOrigin] value: Int):
    print(value)
def mutate_unsafe(ref[MutUnsafeAnyOrigin] value: Int):
    value += 1
def main():
    var p = unsafe_alloc[Int](1)
    p[] = 41
    observe_untracked(p[])
    mutate_unsafe(p[])
    print(p[])
    p.free()
