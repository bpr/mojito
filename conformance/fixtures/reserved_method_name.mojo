@fieldwise_init
struct Matcher:
    var value: Int

    def match(self) -> Int:
        return self.value

def main():
    var matcher = Matcher(1)
    print(matcher.match())
