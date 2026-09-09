# A transferred loan rooted at a caller-owned parameter place may leave with
# the returned collection: parameter origins do not escape.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

def fill(mut source: List[Int]) -> List[RefBox[origin_of(source)]]:
    var sink = List[RefBox]()
    ref alias = source
    sink.append(RefBox(Pointer(to=alias)))
    return sink^

def main():
    var keep: List[Int] = [4]
    var got = fill(keep)
    print(len(got))
