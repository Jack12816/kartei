# frozen_string_literal: true

# A user is the root element of the account domain and holds all
# mandatory information for the authentication.
class User < ApplicationRecord
  has_many :sessions

  # Build the display name from the first and last name.
  #
  # @return [String] the display name
  def display_name
    "#{first_name} #{last_name}"
  end
end
