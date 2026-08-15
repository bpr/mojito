# Mojito accepts the experimental `._subtree` origin projection in Pointer
# origin arguments and `unsafe_origin_cast` targets (a deliberate bridge to
# upstream's #lit.origin.subtree experiment); the audited head fails this
# shape in its pass manager ("use of a never-initialized interior
# reference").
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return Pointer(to=self.value).unsafe_origin_cast[
            origin_of(self)._subtree
        ]()

def main():
    var b = Buf(3)
    var p = b.view()
    p[] = 4
    print(b.value)
