# expect: access to 'local' conflicts with live reference 'k'
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Keeper:
    var slot: RefBox

    def add_param(mut self, var box: RefBox):
        def push(var b: RefBox) {mut self}:
            self.slot = b^
        push(box^)

def main():
    var keep: List[Int] = [1]
    ref whole = keep
    var k = Keeper(RefBox(whole))
    var local: List[Int] = [9]
    ref alias = local
    k.add_param(RefBox(alias))
    local.append(1)
    print(k.slot.value[0])
