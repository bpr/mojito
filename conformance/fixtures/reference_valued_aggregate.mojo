@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] Int


def main():
    var value = 40
    ref view = value
    var box = RefBox(view)
    box.value += 2
    print(value)
