# A raw (untracked) pointer read after its storage is freed diagnoses
# deterministically in the VM. (The tracked `Allocation.unsafe_ptr()`
# spelling of this program now rejects statically — the moved owner
# invalidates the pointer's interior generation — pinned by
# tests/vm_test.rs::allocation_tracked_pointer_stales_on_dealloc; the
# untracked escape hatch keeps the runtime backstop.)
# expect: use after Pointer deallocation
from std.memory import Layout, dealloc

def main():
    var allocation = alloc(Layout[Int](count=1))
    var raw = allocation^.unsafe_leak()
    raw.unsafe_write(1)
    raw.unsafe_free()
    print(raw[])
