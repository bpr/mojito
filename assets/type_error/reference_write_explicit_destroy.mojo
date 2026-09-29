# expect: 'r' abandoned without being explicitly destroyed: call close()
# Writing through a `ref` binding destroys the referent's old value, which an
# `@explicit_destroy` type forbids.
@explicit_destroy("call close()")
struct Res(Movable, Deinitable where False):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def close(deinit self):
        pass


def main():
    var a = Res(1)
    ref r = a
    r = Res(2)
    a^.close()
