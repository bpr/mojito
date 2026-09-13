# A nested `def` stores a carrier over one origin into a struct field typed
# at another. Mojito's origin analysis proves the store safe; the pin's type
# rule wants the two origins equal, and its exclusivity rule then refuses the
# capture list as well. The `ref`-field spelling of the same program lives at
# assets/extensions/ok/captured_store_local_carrier.mojo.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

@fieldwise_init
struct Carrier[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var sink = Carrier(RefBox(Pointer(to=whole)))
    var local: List[Int] = [9]
    def push() {mut sink, mut local}:
        ref view = local
        sink.slot = RefBox(Pointer(to=view))
    push()
    print(sink.slot.value[][0])
