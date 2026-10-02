# expect: cannot transfer out of immutable reference
# A field of a read `self` is no more transferable than `self` itself.
@fieldwise_init
struct Box(Movable):
    var s: String

    def take(self) -> String:
        return self.s^


def main():
    var b = Box(String("a"))
    print(b.take())
