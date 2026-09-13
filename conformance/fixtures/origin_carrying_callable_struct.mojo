# A callable struct over an origin-carrying element type. Mojito erases the
# origin parameter and lets the collection and the box reach one call; the pin
# needs the origin bound and then rejects the call as aliasing.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

@fieldwise_init
struct Stasher(def(mut List[RefBox], RefBox)):
    var count: Int
    def __call__(mut self, mut sink: List[RefBox], box: RefBox):
        self.count += 1
        sink.append(box^)

def main():
    var s = Stasher(0)
    var sink = List[RefBox]()
    var local: List[Int] = [9]
    ref view = local
    s(sink, RefBox(Pointer(to=view)))
    print(sink[0].value[][0])
    local.append(1)
    print(local[1])
