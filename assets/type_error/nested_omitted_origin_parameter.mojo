# expect: 'RefBox[_]' is not concrete
# A parameter annotation may leave its outermost origin slot to inference,
# but an origin-parameterized struct nested as a type argument must bind its
# slot, as at the pin.
@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: Pointer[List[Int], Self.origin]

def stash(mut sink: List[RefBox], var box: RefBox):
    sink.append(box^)

def main():
    print(1)
