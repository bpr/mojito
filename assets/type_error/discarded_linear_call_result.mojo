# `_ = e` destroys its value implicitly, so a call result whose type is not
# `Deinitable` is abandoned there, as a discarded statement expression is.
# expect: '(expression temporary)' abandoned without being explicitly destroyed: close me
@explicit_destroy("close me")
struct Lin(Deinitable where False, Movable):
    var x: Int

    def __init__(out self):
        self.x = 1

    def close(deinit self):
        pass


def mk() -> Lin:
    return Lin()


def main():
    _ = mk()
