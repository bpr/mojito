def main():
    var values = [10, 20, 30]
    ref first = values[0]
    ref same = values[0]

    print(len(values))
    same += 1
    values[0] = 77
    print(first, same)

    values.append(40)
    ref fresh = values[3]
    print(fresh)
