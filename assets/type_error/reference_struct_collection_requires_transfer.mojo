# expect: cannot copy non-Copyable type 'RefBox'
# A collection of reference-bearing structs cannot grow by copy: the element
# carries a loan-bearing handle, so only an explicit transfer could move it.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def stash(mut sink: List[RefBox], box: RefBox):
    sink.append(box)

def main():
    print(1)
