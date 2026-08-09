# expect: access to 'local' conflicts with live reference 's'
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Keeper(def(RefBox)):
    var slot: RefBox
    def __call__(mut self, box: RefBox):
        self.slot = box^

def main():
    var keep = [1]
    ref whole = keep
    var s = Keeper(RefBox(whole))
    var local = [9]
    ref alias = local
    s(RefBox(alias))
    local.append(1)
    print(s.slot.value[0])
