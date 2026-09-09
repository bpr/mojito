@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

def main():
    var sink = List[RefBox]()
    var local: List[Int] = [9]
    ref alias = local
    sink.append(RefBox(Pointer(to=alias)))
    print(sink[0].value[][0])
