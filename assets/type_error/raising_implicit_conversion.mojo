# expect: @implicit requires a non-raising
struct Fallible:
    var value: Int

    @implicit
    def __init__(out self, value: Int) raises:
        self.value = value

def main():
    pass
