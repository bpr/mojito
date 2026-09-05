# Back-to-front iteration: a List's `__reversed__()` iterator driving a loop
# and `reversed(...)` over the three range shapes.
def main():
    var xs: List[Int] = [10, 20, 30]
    for x in xs.__reversed__():
        print(x)
    for i in reversed(range(3)):
        print(i)
    for i in reversed(range(1, 4)):
        print(i)
    for i in reversed(range(0, 10, 4)):
        print(i)
