struct Wrapped:
    var value: Int

    @implicit
    def __init__(out self, value: Int):
        self.value = value

@fieldwise_init
struct Window:
    def __getitem__(self, index: Wrapped) -> Int:
        return index.value

def main():
    var window = Window()
    print(window[13])
