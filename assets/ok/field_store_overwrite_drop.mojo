# Question: storing into a live droppable field.
# Upstream (`1.1.0.dev2026082605`) destroys the replaced value at the
# store: `del 2`, then `del 1` and `del 3` when `p` dies after its last
# use, then `after`.
#
# Mojito today never destroys Inner 2 on the VM: `del 1`, `del 3`,
# `after`. Drop elaboration emits no destruction of the value a field
# `Store` replaces.
#
# On the fix: promote this file to `assets/ok/field_store_overwrite_drop.mojo`
# with its manifest rows, and delete the `field-store-overwrite-drop`
# ledger row in docs/roadmap.md.
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

def main():
    var p = Pair(Inner(1), Inner(2))
    p.b = Inner(3)
    print("after")
