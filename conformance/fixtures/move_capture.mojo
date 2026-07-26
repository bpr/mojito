def main():
    var values = [40]

    def bump() {var values^} -> Int:
        values[0] += 1
        return values[0]

    print(bump())
    print(bump())
