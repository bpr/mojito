# A reference-bearing struct rooted at a caller-owned parameter place may be
# stored into `self`: the loan's origin outlives the frame, so the store is
# not an escape.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Holder:
    var slot: RefBox
    def rebind_to(mut self, mut source: List[Int]):
        ref alias = source
        self.slot = RefBox(alias)

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var holder = Holder(RefBox(whole))
    var other: List[Int] = [5]
    holder.rebind_to(other)
    print(1)
