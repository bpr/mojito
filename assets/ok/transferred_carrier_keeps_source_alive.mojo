@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def main():
    var sink: List[RefBox] = List[RefBox]()
    var local = [9]
    ref alias = local
    sink.append(RefBox(alias))
    print(sink[0].value[0])
