# A `MutOrigin` pointer field is writable from a plain `self` receiver with
# `+=` too: the dereference takes the pointer's capability, not the
# receiver's (upstream runs it).
@fieldwise_init
struct Q[o: MutOrigin]:
    var src: Pointer[List[Int], Self.o]

    def bump(self):
        self.src[][0] += 1

def main():
    var xs = List[Int]()
    xs.append(7)
    var q = Q(Pointer(to=xs))
    q.bump()
    q.src[][0] = 9
    print(xs[0])
