# expect: escapes storage
# Rebinding a `ref` field to a method-local place stores a handle that
# outlives its referent even though the assigned value itself carries no loan.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]
    def refresh(mut self):
        var local = [9]
        self.value = local

def main():
    var values = [1, 2]
    ref whole = values
    var box = RefBox(whole)
    box.refresh()
    print(box.value[0])
