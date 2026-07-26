from std.builtin.builtin_slice import ContiguousSlice

struct Wrapped:
    var value: ContiguousSlice

    @implicit
    def __init__(out self, value: ContiguousSlice):
        self.value = value

@fieldwise_init
struct Window:
    var size: Int

    def __getitem__(self, part: Wrapped) -> Int:
        return part.value.indices(self.size)[1]

def main():
    var window = Window(10)
    print(window[1:4])
