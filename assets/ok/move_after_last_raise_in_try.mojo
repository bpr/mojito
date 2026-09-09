# Pinned against Mojo 1.1.0.dev2026082605 (2026-09-08): the `except` arm's
# state is the join of the states at the body's raise points, so a value
# consumed after the body's only raising call is still initialized in the
# handler (`caught x`); the body then completes and drops nothing twice.
def boom() raises:
    raise Error("boom")


def take(var s: String):
    print("took", s)


def main():
    var s = String("x")
    try:
        boom()
        take(s^)
    except e:
        print("caught", s)
