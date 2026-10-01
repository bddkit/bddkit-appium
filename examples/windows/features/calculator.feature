Feature: Calculator, driven through Appium

  Scenario: add two numbers
    When I tap "one"
    And I tap "plus"
    And I tap "seven"
    And I tap "equals"
    Then the "result" text on the screen should be "Display is 8"
    When I read the "result" text from the screen as "shown"
    Then variable "shown" should be equal to "Display is 8"

  Scenario: enter is equals
    When I tap "seven"
    And I tap "plus"
    And I tap "one"
    And I press the "enter" key
    Then the "result" text on the screen should be "Display is 8"
    When I tap "clear"
    Then the "result" text on the screen should be "Display is 0"
    And I capture the screen
