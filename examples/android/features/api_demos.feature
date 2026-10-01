Feature: ApiDemos, driven through Appium

  Scenario: type into a text field and read it back
    When I tap "app"
    And I tap "alert dialogs"
    And I tap "text entry dialog"
    And I type "hello" into "name field"
    Then the "name field" text on the screen should be "hello"
    When I read the "name field" text from the screen as "typed"
    Then variable "typed" should be equal to "hello"
    When I clear the "name field" field
    And I dump the screen
    And I tap "ok button"
    And I expect the next assertion to pass within "5" seconds
    Then the "name field" should not be on the screen

  Scenario: back leaves the list
    When I tap "app"
    Then the "alert dialogs" should be on the screen
    When I press the "back" key
    And I expect the next assertion to pass within "5" seconds
    Then the "alert dialogs" should not be on the screen
    And the "animation" should be on the screen
    When I capture the screen
