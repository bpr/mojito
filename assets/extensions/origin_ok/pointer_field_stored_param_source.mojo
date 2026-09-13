# Storing a loan rooted at a `mut` parameter into `self`: Mojito's escape
# analysis accepts it, the pin's type rule rejects the origin mismatch, and
# widening to a union origin only moves the rejection to the call's
# exclusivity check. The `ref`-field spelling lives at
# assets/extensions/origin_ok/stored_reference_field_param_source.mojo.
# A pointer-bearing struct rooted at a caller-owned parameter place may be
# stored into `self`: the loan's origin outlives the frame, so the store is
# not an escape.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

@fieldwise_init
struct Holder[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]
    def rebind_to(mut self, mut source: List[Int]):
        ref view = source
        self.slot = RefBox(Pointer(to=view))

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var holder = Holder(RefBox(Pointer(to=whole)))
    var other: List[Int] = [5]
    holder.rebind_to(other)
    print(1)
