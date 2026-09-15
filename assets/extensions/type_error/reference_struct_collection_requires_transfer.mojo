# expect: cannot copy non-Copyable type 'RefBox'
# A collection of reference-bearing structs cannot grow by copy: the element
# carries a loan-bearing handle, so only an explicit transfer could move it.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] List[Int]

def stash[o: Origin[mut=True]](mut sink: List[RefBox[o]], box: RefBox[o]):
    sink.append(box)

def main():
    print(1)
