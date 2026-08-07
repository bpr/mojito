# expect: escapes storage
# Storing a reference-bearing struct rooted at a method-local into a field of
# `self` would leave the field's handle dangling after the frame dies — the
# store-outward twin of the returned-reference escape.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Holder:
    var slot: RefBox
    def swap(mut self):
        var local = [9]
        ref alias = local
        self.slot = RefBox(alias)

def main():
    var keep = [1]
    ref whole = keep
    var holder = Holder(RefBox(whole))
    holder.swap()
    print(holder.slot.value[0])
