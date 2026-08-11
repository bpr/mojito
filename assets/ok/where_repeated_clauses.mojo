@fieldwise_init
struct Box[n: Int] where (n > 0, "positive size") where (n < 10, "small size"):
    var value: Int


def scaled[n: Int](base: Int) -> Int where (n > 0, "positive scale") where (n < 100, "bounded scale"):
    return base * n


def main():
    var box = Box[3](7)
    print(box.value)
    print(scaled[4](5))
