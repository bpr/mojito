struct Distance:
    var value: Int

    @implicit
    def __init__(out self, value: Int):
        self.value = value

def display(distance: Distance):
    print(distance.value)

def make_distance() -> Distance:
    return 8

def main():
    var first: Distance = 6
    display(7)
    var second = make_distance()
    print(first.value, second.value)
