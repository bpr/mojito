# A transferred loan rooted at a caller-owned parameter place may leave with
# the returned collection: parameter origins do not escape.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def fill(mut source: List[Int]) -> List[RefBox[origin_of(source)]]:
    var sink = List[RefBox]()
    ref alias = source
    sink.append(RefBox(alias))
    return sink^

def main():
    var keep: List[Int] = [4]
    var got = fill(keep)
    print(len(got))
