Feature: the whole vocabulary against a fake Appium

  Scenario: act on the screen and read it back
    When I tap "cart button"
    And I type "shoes" into "search field"
    Then the "search field" text on the screen should be "shoes"
    When I read the "search field" text from the screen as "typed"
    Then variable "typed" should be equal to "shoes"
    When I clear the "search field" field
    Then the "search field" text on the screen should be ""
    When I press the "back" key
    And I capture the screen
    And I dump the screen
    Then the "cart button" should be on the screen
    And the "promo banner" should not be on the screen

  Scenario: wait for an element through the host's polling
    When I expect the next assertion to pass within "5" seconds
    Then the "late banner" should be on the screen

  Scenario: a variable reaches the plugin interpolated
    Given set variable "query" to "boots"
    When I type "<<query>>" into "search field"
    Then the "search field" text on the screen should be "boots"
