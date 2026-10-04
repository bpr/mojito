# A constructor's `comptime if` must initialize every field in every arm: the
# template's check treats the condition as opaque, as the pin does, so an arm
# no instance takes still fails.
# expect: does not initialize field 'x'
struct Box[T: Copyable & Deinitable & Defaultable]:
    var x: Self.T

    def __init__(out self):
        comptime if Self.T == Int:
            self.x = Self.T()
        else:
            pass


def main():
    var b = Box[Int]()
    print(b.x)
