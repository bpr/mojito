# A thin callable parameter called through with an origin-carrying argument.
# Mojito erases the boxes' origin parameter and takes a read parameter as a
# transferable place; the pin demands a concrete origin, wants `var box` for
# the transfer, and then refuses the call as aliasing. The `ref`-field
# spelling lives at assets/extensions/ok/funarg_call_through_after_last_use.mojo.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

@fieldwise_init
struct Carrier[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

def stash(mut sink: Carrier, box: RefBox):
    sink.slot = box^

def feed[callback: def(mut Carrier, RefBox) thin](mut sink: Carrier, box: RefBox):
    callback(sink, box^)

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var sink = Carrier(RefBox(Pointer(to=whole)))
    var local: List[Int] = [9]
    ref view = local
    feed[stash](sink, RefBox(Pointer(to=view)))
    print(sink.slot.value[][0])
    local.append(1)
    print(local[1])
