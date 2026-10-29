# A generic struct's construction applies its constructor at a parameter
# typed by the struct's own parameter (`v: Self.T`), whether it spells its
# instance (`Box[Int](6)`) or the run-time construction infers it
# (`Box(5)`), as at the pin.
@fieldwise_init
struct Box[T: ImplicitlyCopyable & Deinitable](ImplicitlyCopyable):
    var v: Self.T


struct TB[b: Box[Int]]:
    def __init__(out self):
        pass

    def show(self):
        print(Self.b.v * 2)


def main():
    TB[Box[Int](6)]().show()
    TB[Box(5)]().show()
