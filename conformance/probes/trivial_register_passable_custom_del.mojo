# Probe: a `TrivialRegisterPassable` struct declares `__del__`.
#
# The pin (2026-10-14) rejects it: "trivial types must not declare '__del__'
# methods; they are trivially destroyable". Mojito accepts it and prints `1`
# then `del`. docs/roadmap.md R860.
struct P(TrivialRegisterPassable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __del__(deinit self):
        print("del")


def main():
    var p = P(1)
    print(p.x)
