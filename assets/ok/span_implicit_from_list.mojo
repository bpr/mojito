# Temporary-origin inference (current Mojo): a List passes directly where
# a Span parameter is expected. The @implicit view constructor's `ref
# [origin]` parameter borrows the argument, the temporary's origin refines
# to the source list, and the hidden retained slot keeps the List alive
# across the consuming call. Free calls, method calls, and annotated
# bindings all convert.
@fieldwise_init
struct Reader:
    var offset: Int

    def read(self, s: Span[Int]) -> Int:
        return s[self.offset]

def total(s: Span[Int]) -> Int:
    var acc = 0
    var i = 0
    while i < len(s):
        acc += s[i]
        i += 1
    return acc

def main():
    var xs = List[Int]()
    xs.append(10)
    xs.append(20)
    print(total(xs))
    var r = Reader(1)
    print(r.read(xs))
    var s: Span[Int] = xs
    print(s[0])
