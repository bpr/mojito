# expect: access to 'local' conflicts with live reference 's'
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Keeper[origin: Origin[mut=True]](def(RefBox)):
    var slot: RefBox[Self.origin]
    def __call__(mut self, box: RefBox):
        self.slot = box^

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var s = Keeper(RefBox(whole))
    var local: List[Int] = [9]
    ref alias = local
    s(RefBox(alias))
    local.append(1)
    print(s.slot.value[0])
