# expect: invalidated interior reference
# The subtree rule is stricter than named interior generations: mutating a
# SUBFIELD of the base stales a subtree reference rooted at the base, because
# the reference may designate that very descendant.
@fieldwise_init
struct Buf:
    var value: Int
    var other: Int

    def view(ref self) -> Pointer[Int, origin_of(self)._subtree]:
        return UnsafePointer(to=self.value).origin_cast[
            origin_of(self)._subtree
        ]()

def main():
    var b = Buf(3, 4)
    var p = b.view()
    b.other = 9
    print(p[])
