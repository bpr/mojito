# expect: 'tx' abandoned without being explicitly destroyed: call commit or rollback
# `_ = tx^` discards an `@explicit_destroy` value without a named destructor:
# abandonment, with upstream's text and the struct's own message.
@explicit_destroy("call commit or rollback")
struct Tx(Movable, Deinitable where False):
    var n: Int

    def __init__(out self):
        self.n = 1

    def commit(deinit self):
        print("commit")


def main():
    var tx = Tx()
    _ = tx^
