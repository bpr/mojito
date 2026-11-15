# A struct that opts out of `Deinitable` with a reason (`not Deinitable
# else "..."`) must be explicitly destroyed, and abandoning one reports
# that reason.
# expect: 's' abandoned without being explicitly destroyed: call close() instead


struct Session(Movable, not Deinitable else "call close() instead"):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        pass


def main():
    var s = Session(3)
    print(s.id)
