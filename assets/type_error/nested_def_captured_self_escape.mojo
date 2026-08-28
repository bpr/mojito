# expect: stored reference escapes storage
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Holder[origin: Origin[mut=True]]:
    var slot: RefBox[Self.origin]

    def stash_local(mut self):
        def install() {mut self}:
            var local: List[Int] = [7]
            ref alias = local
            self.slot = RefBox(alias)
        install()

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var holder = Holder(RefBox(whole))
    holder.stash_local()
    print(holder.slot.value[0])
