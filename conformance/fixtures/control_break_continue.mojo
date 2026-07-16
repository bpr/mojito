def main():
    var total: Int = 0
    for i in range(8):
        if i == 6:
            break
        if i % 2 == 0:
            continue
        total += i
    print(total)
