# Pinned against Mojo 1.2.0.dev2026101705 (2026-10-23): a field the body
# replaces dies as soon as nothing reads it, even when a sibling field is
# read first through a register (`p.b.id` loaded into a call) or through a
# reference (`ref r = p.b`): the read keeps only the part it reaches alive,
# as upstream's `DestructorInsertion` follows a register's and a loan's
# part (`getValueRefsForAccess`), so the replaced `Inner(1)` dies right
# after `p` is built.
# Expected:
#   del inner 1 / built / b is 2 / end 3 / del inner 3 / del inner 2
#   del inner 1 / built / r is 2 / end 3 / del inner 3 / del inner 2
#   done
struct Inner:
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("del inner", self.id)


struct Pair:
    var a: Inner
    var b: Inner

    def __init__(out self, var a: Inner, var b: Inner):
        self.a = a^
        self.b = b^


def through_register():
    var p = Pair(Inner(1), Inner(2))
    print("built")
    print("b is", p.b.id)
    p.a = Inner(3)
    print("end", p.a.id)


def through_reference():
    var p = Pair(Inner(1), Inner(2))
    print("built")
    ref r = p.b
    print("r is", r.id)
    p.a = Inner(3)
    print("end", p.a.id)


def main():
    through_register()
    through_reference()
    print("done")
