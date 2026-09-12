@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

@fieldwise_init
struct Holder[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

    def stash_param(mut self, mut source: List[Int]):
        def install() {mut self, ref source}:
            ref view = source
            self.slot = RefBox(Pointer(to=view))
        install()

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var holder = Holder(RefBox(Pointer(to=whole)))
    var other: List[Int] = [5]
    holder.stash_param(other)
    print("stored")
