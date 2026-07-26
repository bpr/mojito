def alter(mut values: List[Int]):
    values.append(40)


def main():
    var values = [10, 20, 30]
    ref first = values[0]
    alter(values)
    print(first)
