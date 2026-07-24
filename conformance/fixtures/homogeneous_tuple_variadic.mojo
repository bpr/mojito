def show(*items: Tuple[Int, Int]):
    print(len(items))
    print(items[0][1])
    print(items[1][0])


def main():
    show((1, 2), (3, 4))
