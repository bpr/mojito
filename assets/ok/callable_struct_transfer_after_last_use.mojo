@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

@fieldwise_init
struct Stasher(def(mut List[RefBox], RefBox)):
    var count: Int
    def __call__(mut self, mut sink: List[RefBox], box: RefBox):
        self.count += 1
        sink.append(box^)

def main():
    var s = Stasher(0)
    var sink: List[RefBox] = List[RefBox]()
    var local = [9]
    ref alias = local
    s(sink, RefBox(alias))
    print(sink[0].value[0])
    local.append(1)
    print(local[1])
