# expect: does not match the signature required by trait
# A witness for a `*args` collector requirement must declare the collector.
trait Taker:
    def take[*Ts: Writable](self, *a: *Ts):
        ...


struct Bad(Taker):
    def __init__(out self):
        pass

    def take(self, a: Int):
        print(a)


def main():
    Bad().take(1)
