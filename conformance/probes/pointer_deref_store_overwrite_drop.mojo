# Question: storing through a pointer dereference over a live droppable value.
# Upstream (`1.1.0.dev2026082605`) destroys the replaced value at the store:
# `del 1`, then `after 2`, then `del 2`.
#
# Mojito today never destroys Inner 1 on the VM: `after 2`, `del 2`. Drop
# elaboration destroys the value a store replaces only for a static field or
# element place, and `q[] = …` is an indexed place below the pointer.
#
# On the fix: promote this file to
# `assets/ok/pointer_deref_store_overwrite_drop.mojo` with its manifest rows,
# and delete the `pointer-deref-store-overwrite-drop` ledger row in
# docs/roadmap.md.
@fieldwise_init
struct Inner(Movable):
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

def main():
    var b = Inner(1)
    var q = Pointer(to=b)
    q[] = Inner(2)
    print("after", b.id)
