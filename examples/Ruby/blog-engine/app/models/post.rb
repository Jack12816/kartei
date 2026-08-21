# frozen_string_literal: true

# A blog post written by an author.
class Post < ApplicationRecord
  belongs_to :author

  # Publish the post now.
  def publish!
    update!(published_at: Time.current)
  end
end
