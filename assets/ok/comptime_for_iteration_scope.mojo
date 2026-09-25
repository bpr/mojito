# Each unrolled iteration of a `comptime for` is its own scope, as upstream's:
# a `var` in the body is one binding per iteration, may shadow a local of the
# enclosing block, and an owning one is destroyed within its own iteration. A
# selected `comptime if` arm is a scope too, so its `var` does not collide
# with a later one of the same name.
@fieldwise_init
struct Noisy(Movable):
    var v: Int

    def __deinit__(deinit self):
        print("del", self.v)


def arm[n: Int]() -> Int:
    comptime if n > 0:
        var x = n
        print(x)
    var x = 2
    return x


def main():
    var sum = 0
    comptime for i in range(3):
        var step = i * 2
        sum += step
    print(sum)
    var x = 100
    comptime for i in range(2):
        var x = i
        x += 1
        print(x)
    print(x)
    comptime for i in range(2):
        var n = Noisy(i)
        print("made", n.v)
    print("end")
    print(arm[1](), arm[0]())
