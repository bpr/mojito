# `size_of[T]()` uses the shared native ABI layout, including aggregate padding.
@fieldwise_init
struct Padded:
    var flag: Bool
    var value: Int

def generic_size[T: AnyType]() -> Int:
    return size_of[T]()

def main():
    print(size_of[NoneType]())
    print(size_of[Bool](), size_of[Int]())
    print(size_of[Padded]())
    print(generic_size[Padded]())
# stdout: 0
# stdout: 1 8
# stdout: 16
# stdout: 16
